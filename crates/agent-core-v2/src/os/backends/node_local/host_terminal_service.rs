//! `portable-pty` backed host terminal process factory.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/hostTerminalService.ts`.

use std::{
    io::{Read, Write},
    sync::{Arc, Mutex, Weak},
};

use async_trait::async_trait;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::{
    _base::{
        di::lifecycle::{Disposable, DisposeResult},
        errors::unexpected_error::on_unexpected_error,
        event::{Emitter, Event},
    },
    os::interface::terminal::{
        HostTerminalService, TerminalProcess, TerminalProcessError, TerminalProcessExit,
        TerminalSpawnOptions,
    },
};

struct SpawnedPty {
    master: Box<dyn MasterPty + Send>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

pub struct LocalTerminalProcess {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    data: Arc<Emitter<String>>,
    exit: Arc<Emitter<TerminalProcessExit>>,
}

impl TerminalProcess for LocalTerminalProcess {
    fn on_process_data(&self) -> Event<String> {
        self.data.event()
    }
    fn on_process_exit(&self) -> Event<TerminalProcessExit> {
        self.exit.event()
    }

    fn write(&self, data: &str) -> Result<(), TerminalProcessError> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(data.as_bytes()).map_err(terminal_error)?;
        writer.flush().map_err(terminal_error)
    }

    fn resize(&self, cols: u32, rows: u32) -> Result<(), TerminalProcessError> {
        self.master
            .lock()
            .unwrap()
            .resize(pty_size(cols, rows)?)
            .map_err(terminal_error)
    }

    fn kill(&self) -> Result<(), TerminalProcessError> {
        self.killer.lock().unwrap().kill().map_err(terminal_error)
    }
}

#[derive(Default)]
pub struct LocalHostTerminalService {
    processes: Mutex<Vec<Weak<LocalTerminalProcess>>>,
}

#[async_trait]
impl HostTerminalService for LocalHostTerminalService {
    async fn spawn(
        &self,
        options: TerminalSpawnOptions,
    ) -> Result<Arc<dyn TerminalProcess>, TerminalProcessError> {
        let size = pty_size(options.cols, options.rows)?;
        let spawned = tokio::task::spawn_blocking(move || spawn_pty(options, size))
            .await
            .map_err(terminal_error)??;
        let child = spawned.child;
        let killer = child.clone_killer();
        let data = Arc::new(Emitter::new());
        let exit = Arc::new(Emitter::new());
        spawn_reader(spawned.reader, Arc::clone(&data));
        spawn_waiter(child, Arc::clone(&exit));
        let process = Arc::new(LocalTerminalProcess {
            master: Mutex::new(spawned.master),
            writer: Mutex::new(spawned.writer),
            killer: Mutex::new(killer),
            data,
            exit,
        });
        self.processes
            .lock()
            .unwrap()
            .push(Arc::downgrade(&process));
        Ok(process)
    }
}

impl Disposable for LocalHostTerminalService {
    fn dispose(&self) -> DisposeResult {
        for process in self
            .processes
            .lock()
            .unwrap()
            .drain(..)
            .filter_map(|process| process.upgrade())
        {
            let _ = process.kill();
        }
        Ok(())
    }
}

impl Drop for LocalHostTerminalService {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

fn spawn_pty(
    options: TerminalSpawnOptions,
    size: PtySize,
) -> Result<SpawnedPty, TerminalProcessError> {
    let pair = native_pty_system().openpty(size).map_err(terminal_error)?;
    let mut command = CommandBuilder::new(options.shell);
    command.cwd(options.cwd);
    let child = pair.slave.spawn_command(command).map_err(terminal_error)?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(terminal_error)?;
    let writer = pair.master.take_writer().map_err(terminal_error)?;
    Ok(SpawnedPty {
        master: pair.master,
        reader,
        writer,
        child,
    })
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, emitter: Arc<Emitter<String>>) {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut pending = Vec::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    pending.extend_from_slice(&buffer[..count]);
                    emit_utf8(&mut pending, &emitter, false);
                }
                Err(error) => {
                    on_unexpected_error(&error);
                    break;
                }
            }
        }
        emit_utf8(&mut pending, &emitter, true);
    });
}

fn emit_utf8(pending: &mut Vec<u8>, emitter: &Emitter<String>, eof: bool) {
    loop {
        match std::str::from_utf8(pending) {
            Ok(text) => {
                if !text.is_empty() {
                    emitter.fire(&text.to_owned());
                }
                pending.clear();
                return;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let text = String::from_utf8(pending.drain(..valid).collect()).unwrap();
                    emitter.fire(&text);
                    continue;
                }
                if let Some(length) = error.error_len() {
                    pending.drain(..length);
                    emitter.fire(&char::REPLACEMENT_CHARACTER.to_string());
                    continue;
                }
                if eof {
                    let text = String::from_utf8_lossy(pending).into_owned();
                    if !text.is_empty() {
                        emitter.fire(&text);
                    }
                    pending.clear();
                }
                return;
            }
        }
    }
}

fn spawn_waiter(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    emitter: Arc<Emitter<TerminalProcessExit>>,
) {
    std::thread::spawn(move || match child.wait() {
        Ok(status) => emitter.fire(&TerminalProcessExit {
            exit_code: i32::try_from(status.exit_code()).ok(),
        }),
        Err(error) => on_unexpected_error(&error),
    });
}

fn pty_size(cols: u32, rows: u32) -> Result<PtySize, TerminalProcessError> {
    Ok(PtySize {
        rows: u16::try_from(rows)
            .map_err(|_| TerminalProcessError("terminal rows exceed PTY limit".into()))?,
        cols: u16::try_from(cols)
            .map_err(|_| TerminalProcessError("terminal cols exceed PTY limit".into()))?,
        pixel_width: 0,
        pixel_height: 0,
    })
}

fn terminal_error(error: impl std::fmt::Display) -> TerminalProcessError {
    TerminalProcessError(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn spawns_writes_resizes_emits_output_and_exit() {
        let service = LocalHostTerminalService::default();
        let process = service
            .spawn(TerminalSpawnOptions {
                cwd: std::env::temp_dir().to_string_lossy().into_owned(),
                shell: "/bin/sh".into(),
                cols: 80,
                rows: 24,
            })
            .await
            .unwrap();
        let (data_tx, mut data_rx) = mpsc::unbounded_channel();
        let _data_subscription = process.on_process_data().subscribe(move |data| {
            let _ = data_tx.send(data.clone());
        });
        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel();
        let _exit_subscription = process.on_process_exit().subscribe(move |exit| {
            let _ = exit_tx.send(*exit);
        });
        process.resize(100, 30).unwrap();
        process.write("printf __PTY_OK__; exit\n").unwrap();
        let output = tokio::time::timeout(Duration::from_secs(3), async {
            let mut output = String::new();
            while let Some(chunk) = data_rx.recv().await {
                output.push_str(&chunk);
                if output.contains("__PTY_OK__") {
                    break;
                }
            }
            output
        })
        .await
        .unwrap();
        assert!(output.contains("__PTY_OK__"));
        let exit = tokio::time::timeout(Duration::from_secs(3), exit_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(exit.exit_code, Some(0));
    }

    #[test]
    fn rejects_sizes_outside_native_pty_width() {
        assert!(pty_size(u32::from(u16::MAX) + 1, 24).is_err());
    }
}
