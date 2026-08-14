use std::{io, net::SocketAddr, path::PathBuf, sync::Arc};

use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, watch},
    task::JoinHandle,
};

use crate::{
    minidb::{MiniDb, MiniDbError, SetOptions},
    wal::FsyncPolicy,
};

const CRLF: &[u8] = b"\r\n";
type RespCommand = Vec<Vec<u8>>;
type ParsedCommand = Option<(RespCommand, usize)>;
type ServerTask = JoinHandle<Result<(), ServerError>>;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Database(#[from] MiniDbError),
    #[error("server task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("RESP request too large (>{0} bytes)")]
    RequestTooLarge(usize),
    #[error("invalid RESP request: {0}")]
    InvalidRequest(String),
}

pub struct RespParser {
    buffer: Vec<u8>,
    max_buffer: usize,
}

impl Default for RespParser {
    fn default() -> Self {
        Self::new(64 * 1024 * 1024)
    }
}

impl RespParser {
    pub fn new(max_buffer: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max_buffer,
        }
    }

    // Original: packages/minidb/src/server.ts, RespParser.feed()/tryParse().
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Vec<Vec<u8>>>, ServerError> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > self.max_buffer {
            self.buffer.clear();
            return Err(ServerError::RequestTooLarge(self.max_buffer));
        }
        let mut commands = Vec::new();
        while let Some((command, consumed)) = parse_one(&self.buffer)? {
            self.buffer.drain(..consumed);
            commands.push(command);
        }
        Ok(commands)
    }
}

fn parse_one(bytes: &[u8]) -> Result<ParsedCommand, ServerError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes[0] != b'*' {
        let Some(end) = find_crlf(bytes, 0) else {
            return Ok(None);
        };
        let command = bytes[..end]
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|part| !part.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        return Ok(Some((command, end + 2)));
    }
    let Some(end) = find_crlf(bytes, 1) else {
        return Ok(None);
    };
    let count = parse_usize(&bytes[1..end], "array length")?;
    let mut position = end + 2;
    let mut command = Vec::with_capacity(count);
    for _ in 0..count {
        if position >= bytes.len() {
            return Ok(None);
        }
        if bytes[position] != b'$' {
            return Ok(None);
        }
        let Some(end) = find_crlf(bytes, position + 1) else {
            return Ok(None);
        };
        let length = parse_usize(&bytes[position + 1..end], "bulk length")?;
        position = end + 2;
        let Some(payload_end) = position.checked_add(length) else {
            return Err(ServerError::InvalidRequest("bulk length overflow".into()));
        };
        if payload_end + 2 > bytes.len() {
            return Ok(None);
        }
        if &bytes[payload_end..payload_end + 2] != CRLF {
            return Err(ServerError::InvalidRequest("bulk value lacks CRLF".into()));
        }
        command.push(bytes[position..payload_end].to_vec());
        position = payload_end + 2;
    }
    Ok(Some((command, position)))
}

fn find_crlf(bytes: &[u8], start: usize) -> Option<usize> {
    bytes
        .get(start..)?
        .windows(2)
        .position(|window| window == CRLF)
        .map(|relative| start + relative)
}

fn parse_usize(bytes: &[u8], name: &str) -> Result<usize, ServerError> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| ServerError::InvalidRequest(format!("invalid {name}")))
}

fn simple(value: &str) -> Vec<u8> {
    format!("+{value}\r\n").into_bytes()
}
fn integer(value: i64) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}
fn error(value: impl std::fmt::Display) -> Vec<u8> {
    format!("-ERR {value}\r\n").into_bytes()
}
fn bulk(value: Option<&[u8]>) -> Vec<u8> {
    match value {
        None => b"$-1\r\n".to_vec(),
        Some(value) => {
            let mut output = format!("${}\r\n", value.len()).into_bytes();
            output.extend_from_slice(value);
            output.extend_from_slice(CRLF);
            output
        }
    }
}
fn array(values: &[Option<Vec<u8>>]) -> Vec<u8> {
    let mut output = format!("*{}\r\n", values.len()).into_bytes();
    for value in values {
        output.extend_from_slice(&bulk(value.as_deref()));
    }
    output
}

enum CommandResult {
    Reply(Vec<u8>),
    Quit,
}

// Original: server.ts, handle().
async fn handle(
    database: &MiniDb<String>,
    arguments: &[Vec<u8>],
) -> Result<CommandResult, MiniDbError> {
    let command = arguments
        .first()
        .map(|value| String::from_utf8_lossy(value).to_ascii_uppercase())
        .unwrap_or_default();
    let string = |index: usize| {
        arguments
            .get(index)
            .map(|value| String::from_utf8_lossy(value).into_owned())
    };
    let reply = match command.as_str() {
        "PING" => arguments
            .get(1)
            .map_or_else(|| simple("PONG"), |value| bulk(Some(value))),
        "ECHO" => bulk(arguments.get(1).map(Vec::as_slice)),
        "GET" => bulk(
            database
                .get(arguments.get(1).map_or(&[][..], Vec::as_slice))?
                .as_deref()
                .map(str::as_bytes),
        ),
        "SET" => {
            let key = arguments.get(1).map_or(&[][..], Vec::as_slice);
            let value = string(2).unwrap_or_default();
            let mut ttl = None;
            let mut index = 3;
            while index < arguments.len() {
                let option = string(index).unwrap_or_default().to_ascii_uppercase();
                if option == "EX" || option == "PX" {
                    index += 1;
                    let value = string(index)
                        .and_then(|value| value.parse::<u64>().ok())
                        .ok_or(MiniDbError::InvalidTtl)?;
                    ttl = Some(if option == "EX" {
                        value.saturating_mul(1_000)
                    } else {
                        value
                    });
                }
                index += 1;
            }
            database
                .set(
                    key,
                    value,
                    SetOptions {
                        ttl_millis: ttl,
                        ..Default::default()
                    },
                )
                .await?;
            simple("OK")
        }
        "DEL" => {
            let mut deleted = 0;
            for key in &arguments[1..] {
                if database.del(key).await? {
                    deleted += 1;
                }
            }
            integer(deleted)
        }
        "EXISTS" => integer(i64::from(
            database.has(arguments.get(1).map_or(&[][..], Vec::as_slice))?,
        )),
        "MGET" => {
            let mut values = Vec::new();
            for key in &arguments[1..] {
                values.push(database.get(key)?.map(String::into_bytes));
            }
            array(&values)
        }
        "MSET" => {
            let mut entries = Vec::new();
            for pair in arguments[1..].chunks_exact(2) {
                entries.push((
                    String::from_utf8_lossy(&pair[0]).into_owned(),
                    String::from_utf8_lossy(&pair[1]).into_owned(),
                ));
            }
            database.mset(entries).await?;
            simple("OK")
        }
        "TTL" => integer(database.ttl(arguments.get(1).map_or(&[][..], Vec::as_slice))? / 1_000),
        "DBSIZE" => integer(database.len()? as i64),
        "COMPACT" => {
            database.compact().await?;
            simple("OK")
        }
        "INFO" => {
            let compactions = database.compaction_stats()?.compactions;
            bulk(Some(
                format!(
                    "minidb_version:0.0.1\r\nkeys:{}\r\ncompactions:{compactions}\r\n",
                    database.len()?
                )
                .as_bytes(),
            ))
        }
        "QUIT" => return Ok(CommandResult::Quit),
        _ => error(format!("unknown command '{command}'")),
    };
    Ok(CommandResult::Reply(reply))
}

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub directory: PathBuf,
    pub port: u16,
    pub host: String,
    pub fsync_policy: FsyncPolicy,
}

impl ServerOptions {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            port: 6_379,
            host: "127.0.0.1".into(),
            fsync_policy: FsyncPolicy::EverySecond,
        }
    }
}

pub struct ServerHandle {
    pub database: MiniDb<String>,
    pub address: SocketAddr,
    shutdown: watch::Sender<bool>,
    task: Arc<Mutex<Option<ServerTask>>>,
}

impl ServerHandle {
    pub async fn close(&self) -> Result<(), ServerError> {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.lock().await.take() {
            task.await??;
        }
        self.database.close().await?;
        Ok(())
    }
}

// Original: server.ts, startServer().
pub async fn start_server(options: ServerOptions) -> Result<ServerHandle, ServerError> {
    let mut database_options = MiniDb::<String>::string_options(&options.directory);
    database_options.fsync_policy = options.fsync_policy;
    let database = MiniDb::open(database_options).await?;
    let listener = TcpListener::bind((options.host.as_str(), options.port)).await?;
    let address = listener.local_addr()?;
    let (shutdown, mut shutdown_rx) = watch::channel(false);
    let task_database = database.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => { if changed.is_err() || *shutdown_rx.borrow() { break; } }
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let database = task_database.clone();
                    tokio::spawn(async move { let _ = serve_connection(stream, database).await; });
                }
            }
        }
        Ok(())
    });
    Ok(ServerHandle {
        database,
        address,
        shutdown,
        task: Arc::new(Mutex::new(Some(task))),
    })
}

async fn serve_connection(
    mut stream: TcpStream,
    database: MiniDb<String>,
) -> Result<(), ServerError> {
    let mut parser = RespParser::default();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        let commands = match parser.feed(&buffer[..count]) {
            Ok(commands) => commands,
            Err(failure) => {
                stream.write_all(&error(failure)).await?;
                continue;
            }
        };
        for arguments in commands {
            let response = match handle(&database, &arguments).await {
                Ok(response) => response,
                Err(failure) => CommandResult::Reply(error(failure)),
            };
            match response {
                CommandResult::Reply(bytes) => stream.write_all(&bytes).await?,
                CommandResult::Quit => {
                    stream.shutdown().await?;
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_inline_fragmented_and_pipelined_resp() {
        let mut parser = RespParser::new(1024);
        assert!(
            parser
                .feed(b"*2\r\n$3\r\nGET\r\n$3\r\n")
                .unwrap()
                .is_empty()
        );
        let commands = parser.feed(b"key\r\nPING\r\n").unwrap();
        assert_eq!(
            commands,
            vec![
                vec![b"GET".to_vec(), b"key".to_vec()],
                vec![b"PING".to_vec()]
            ]
        );
    }

    #[tokio::test]
    async fn serves_basic_resp_commands_in_order() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = ServerOptions::new(directory.path());
        options.port = 0;
        let server = start_server(options).await.unwrap();
        let mut stream = TcpStream::connect(server.address).await.unwrap();
        stream
            .write_all(b"SET key value\r\nGET key\r\n")
            .await
            .unwrap();
        let mut response = vec![0; 64];
        let count = stream.read(&mut response).await.unwrap();
        assert_eq!(&response[..count], b"+OK\r\n$5\r\nvalue\r\n");
        server.close().await.unwrap();
    }
}
