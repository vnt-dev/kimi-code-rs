use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use crate::os::{
    backends::node_local::tools::rg_locator::RgProbe,
    interface::host_process::{HostProcessOptions, HostProcessServiceHandle},
};

pub(crate) struct ProcessRgProbe {
    process_service: HostProcessServiceHandle,
}

impl ProcessRgProbe {
    pub(crate) fn new(process_service: HostProcessServiceHandle) -> Self {
        Self { process_service }
    }
}

#[async_trait]
impl RgProbe for ProcessRgProbe {
    async fn exec(&self, args: &[String]) -> i32 {
        let Some((command, rest)) = args.split_first() else {
            return -1;
        };
        let Ok(process) = self
            .process_service
            .spawn(command, rest, HostProcessOptions::default())
            .await
        else {
            return -1;
        };
        if let Ok(mut stdin) = process.stdin().try_lock() {
            let _ = stdin.shutdown().await;
        }
        let stdout = process.stdout();
        let stderr = process.stderr();
        let drain_stdout = async move {
            let mut stream = stdout.lock().await;
            let mut sink = tokio::io::sink();
            let _ = tokio::io::copy(&mut *stream, &mut sink).await;
        };
        let drain_stderr = async move {
            let mut stream = stderr.lock().await;
            let mut sink = tokio::io::sink();
            let _ = tokio::io::copy(&mut *stream, &mut sink).await;
        };
        let (_, _, exit) = tokio::join!(drain_stdout, drain_stderr, process.wait());
        process.dispose();
        exit.unwrap_or(-1)
    }
}
