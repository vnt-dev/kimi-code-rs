use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use async_trait::async_trait;

use super::{
    doctor::{DoctorRuntime, DoctorRuntimeError},
    provider_config::validate_provider_config_toml,
};
use crate::utils::paths::get_data_dir;

pub struct SystemDoctorRuntime {
    current_dir: PathBuf,
    data_dir: PathBuf,
    stdout: Mutex<Box<dyn Write + Send>>,
    stderr: Mutex<Box<dyn Write + Send>>,
}

impl SystemDoctorRuntime {
    pub fn new() -> Result<Self, DoctorRuntimeError> {
        let current_dir = std::env::current_dir().map_err(DoctorRuntimeError::new)?;
        let data_dir = get_data_dir().map_err(DoctorRuntimeError::new)?;
        Ok(Self::with_io(
            current_dir,
            data_dir,
            Box::new(std::io::stdout()),
            Box::new(std::io::stderr()),
        ))
    }

    pub fn with_io(
        current_dir: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
    ) -> Self {
        Self {
            current_dir: current_dir.into(),
            data_dir: data_dir.into(),
            stdout: Mutex::new(stdout),
            stderr: Mutex::new(stderr),
        }
    }
}

#[async_trait]
impl DoctorRuntime for SystemDoctorRuntime {
    fn current_dir(&self) -> PathBuf {
        self.current_dir.clone()
    }

    async fn default_config_path(&self) -> Result<PathBuf, DoctorRuntimeError> {
        Ok(self.data_dir.join("config.toml"))
    }

    fn default_tui_config_path(&self) -> Result<PathBuf, DoctorRuntimeError> {
        Ok(self.data_dir.join("tui.toml"))
    }

    fn file_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    async fn read_text_file(&self, path: &Path) -> Result<String, DoctorRuntimeError> {
        tokio::fs::read_to_string(path)
            .await
            .map_err(DoctorRuntimeError::new)
    }

    async fn validate_config_toml(
        &self,
        text: &str,
        file_path: &Path,
    ) -> Result<(), DoctorRuntimeError> {
        validate_provider_config_toml(text, file_path).map_err(DoctorRuntimeError::new)
    }

    fn write_stdout(&self, text: &str) {
        if let Ok(mut stdout) = self.stdout.lock() {
            let _ = stdout.write_all(text.as_bytes());
        }
    }

    fn write_stderr(&self, text: &str) {
        if let Ok(mut stderr) = self.stderr.lock() {
            let _ = stderr.write_all(text.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };

    use super::*;
    use crate::cli::sub::doctor::{DoctorOptions, DoctorTarget, handle_doctor};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl SharedWriter {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().expect("writer").clone()).expect("utf8")
        }
    }

    impl Write for SharedWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("writer").extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn temp_dir() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "kimi-code-rs-doctor-runtime-{}-{id}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn validates_default_config_and_tui_files_end_to_end() {
        let directory = temp_dir();
        tokio::fs::create_dir_all(&directory)
            .await
            .expect("test directory");
        tokio::fs::write(directory.join("config.toml"), "telemetry = false\n")
            .await
            .expect("config fixture");
        tokio::fs::write(directory.join("tui.toml"), "theme = \"dark\"\n")
            .await
            .expect("tui fixture");
        let stdout = SharedWriter::default();
        let stderr = SharedWriter::default();
        let runtime = SystemDoctorRuntime::with_io(
            &directory,
            &directory,
            Box::new(stdout.clone()),
            Box::new(stderr.clone()),
        );

        let code = handle_doctor(&runtime, &DoctorOptions::default())
            .await
            .expect("doctor");

        assert_eq!(code, 0);
        assert!(stdout.text().contains("OK config.toml"));
        assert!(stdout.text().contains("OK tui.toml"));
        assert!(stderr.text().is_empty());
        tokio::fs::remove_dir_all(directory).await.expect("cleanup");
    }

    #[tokio::test]
    async fn reports_structurally_invalid_provider_config() {
        let directory = temp_dir();
        tokio::fs::create_dir_all(&directory)
            .await
            .expect("test directory");
        let path = directory.join("broken.toml");
        tokio::fs::write(&path, "[providers.broken]\ntype = 1\n")
            .await
            .expect("config fixture");
        let stdout = SharedWriter::default();
        let stderr = SharedWriter::default();
        let runtime = SystemDoctorRuntime::with_io(
            &directory,
            &directory,
            Box::new(stdout.clone()),
            Box::new(stderr.clone()),
        );

        let code = handle_doctor(
            &runtime,
            &DoctorOptions {
                target: Some(DoctorTarget::Config),
                path: Some(path),
            },
        )
        .await
        .expect("doctor");

        assert_eq!(code, 1);
        assert!(stdout.text().is_empty());
        assert!(stderr.text().contains("providers.broken"));
        tokio::fs::remove_dir_all(directory).await.expect("cleanup");
    }
}
