use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use ulid::Ulid;
use uuid::Uuid;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(15_000);

/// In-memory shape of a registered server instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInstanceInfo {
    pub server_id: String,
    pub pid: u32,
    pub host: String,
    pub port: u16,
    pub started_at: i64,
    pub heartbeat_at: i64,
    pub host_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServerInstanceDisk {
    server_id: String,
    pid: u32,
    host: String,
    port: u16,
    started_at: i64,
    heartbeat_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_version: Option<String>,
}

impl From<ServerInstanceInfo> for ServerInstanceDisk {
    fn from(info: ServerInstanceInfo) -> Self {
        Self {
            server_id: info.server_id,
            pid: info.pid,
            host: info.host,
            port: info.port,
            started_at: info.started_at,
            heartbeat_at: info.heartbeat_at,
            host_version: info.host_version,
        }
    }
}

impl From<ServerInstanceDisk> for ServerInstanceInfo {
    fn from(info: ServerInstanceDisk) -> Self {
        Self {
            server_id: info.server_id,
            pid: info.pid,
            host: info.host,
            port: info.port,
            started_at: info.started_at,
            heartbeat_at: info.heartbeat_at,
            host_version: info.host_version,
        }
    }
}

#[derive(Clone)]
pub struct InstanceRegistryOptions {
    pub instances_dir: Option<PathBuf>,
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub heartbeat_interval: Duration,
}

impl std::fmt::Debug for InstanceRegistryOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstanceRegistryOptions")
            .field("instances_dir", &self.instances_dir)
            .field("heartbeat_interval", &self.heartbeat_interval)
            .finish_non_exhaustive()
    }
}

impl Default for InstanceRegistryOptions {
    fn default() -> Self {
        Self {
            instances_dir: None,
            now: Arc::new(now_millis),
            heartbeat_interval: HEARTBEAT_INTERVAL,
        }
    }
}

#[derive(Clone)]
pub struct InstanceRegistry {
    instances_dir: PathBuf,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    heartbeat_interval: Duration,
}

impl std::fmt::Debug for InstanceRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstanceRegistry")
            .field("instances_dir", &self.instances_dir)
            .field("heartbeat_interval", &self.heartbeat_interval)
            .finish_non_exhaustive()
    }
}

impl InstanceRegistry {
    pub fn create(options: InstanceRegistryOptions) -> Self {
        let instances_dir = options.instances_dir.unwrap_or_else(|| {
            // MIGRATION-TODO:
            // Original: instanceRegistry.ts, createInstanceRegistry()
            // Missing dependency: agent-core-v2 resolveKimiHome() integration.
            // Completion condition: core-v2 home resolution is declared complete.
            todo!("resolve default instances directory through kimi-code-agent-core-v2")
        });
        Self {
            instances_dir,
            now: options.now,
            heartbeat_interval: options.heartbeat_interval,
        }
    }

    // Original: instanceRegistry.ts, createInstanceRegistry().register().
    pub async fn register(&self, info: RegistrationInfo) -> io::Result<InstanceRegistration> {
        let server_id = Ulid::new().to_string();
        let file_path = self.instances_dir.join(format!("{server_id}.json"));
        fs::create_dir_all(&self.instances_dir).await?;
        sweep_stale(&self.instances_dir).await?;

        let shared = Arc::new(RegistrationShared {
            state: Mutex::new(RegistrationState {
                port: info.port,
                released: false,
            }),
            server_id: server_id.clone(),
            file_path,
            info,
            now: Arc::clone(&self.now),
        });
        write_registration(&shared).await?;

        let (cancel, cancel_rx) = watch::channel(false);
        let heartbeat = spawn_heartbeat(Arc::clone(&shared), self.heartbeat_interval, cancel_rx);
        Ok(InstanceRegistration {
            server_id,
            shared,
            cancel,
            heartbeat: Mutex::new(Some(heartbeat)),
        })
    }

    // Original: instanceRegistry.ts, createInstanceRegistry().listLive().
    pub async fn list_live(&self) -> io::Result<Vec<ServerInstanceInfo>> {
        list_live_internal(&self.instances_dir).await
    }
}

#[derive(Debug, Clone)]
pub struct RegistrationInfo {
    pub pid: u32,
    pub host: String,
    pub port: u16,
    pub started_at: i64,
    pub host_version: Option<String>,
}

#[derive(Debug)]
struct RegistrationState {
    port: u16,
    released: bool,
}

struct RegistrationShared {
    state: Mutex<RegistrationState>,
    server_id: String,
    file_path: PathBuf,
    info: RegistrationInfo,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl std::fmt::Debug for RegistrationShared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegistrationShared")
            .field("server_id", &self.server_id)
            .field("file_path", &self.file_path)
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct InstanceRegistration {
    pub server_id: String,
    shared: Arc<RegistrationShared>,
    cancel: watch::Sender<bool>,
    heartbeat: Mutex<Option<JoinHandle<()>>>,
}

impl InstanceRegistration {
    pub async fn update(&self, port: Option<u16>) -> io::Result<()> {
        let mut state = self.shared.state.lock().await;
        if state.released {
            return Ok(());
        }
        if let Some(port) = port {
            state.port = port;
        }
        write_registration_locked(&self.shared, &state).await
    }

    // Original: InstanceRegistration.release().
    // Holding the state mutex until every write completes prevents a heartbeat
    // rename from recreating the file after removal.
    pub async fn release(&self) -> io::Result<()> {
        {
            let mut state = self.shared.state.lock().await;
            if state.released {
                return Ok(());
            }
            state.released = true;
        }
        let _ = self.cancel.send(true);
        if let Some(heartbeat) = self.heartbeat.lock().await.take() {
            let _ = heartbeat.await;
        }
        match fs::remove_file(&self.shared.file_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

fn spawn_heartbeat(
    shared: Arc<RegistrationShared>,
    interval: Duration,
    mut cancel: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let _ = write_registration(&shared).await;
                }
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

async fn write_registration(shared: &RegistrationShared) -> io::Result<()> {
    let state = shared.state.lock().await;
    if state.released {
        return Ok(());
    }
    write_registration_locked(shared, &state).await
}

async fn write_registration_locked(
    shared: &RegistrationShared,
    state: &RegistrationState,
) -> io::Result<()> {
    let info = ServerInstanceInfo {
        server_id: shared.server_id.clone(),
        pid: shared.info.pid,
        host: shared.info.host.clone(),
        port: state.port,
        started_at: shared.info.started_at,
        heartbeat_at: (shared.now)(),
        host_version: shared.info.host_version.clone(),
    };
    let content = serde_json::to_vec(&ServerInstanceDisk::from(info)).map_err(io::Error::other)?;
    write_file_atomic(&shared.file_path, &content).await
}

async fn write_file_atomic(file_path: &Path, content: &[u8]) -> io::Result<()> {
    let temporary = file_path.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        &Uuid::new_v4().simple().to_string()[..8]
    ));
    let result = async {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await?;
        file.write_all(content).await?;
        drop(file);
        replace_file(&temporary, file_path).await
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result
}

async fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if let Err(error) = fs::remove_file(to).await
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    fs::rename(from, to).await
}

async fn read_instance_file(file_path: &Path) -> Option<ServerInstanceInfo> {
    let raw = fs::read(file_path).await.ok()?;
    serde_json::from_slice::<ServerInstanceDisk>(&raw)
        .ok()
        .map(ServerInstanceInfo::from)
}

async fn instance_paths(instances_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut directory = match fs::read_dir(instances_dir).await {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    while let Some(entry) = directory.next_entry().await? {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

async fn sweep_stale(instances_dir: &Path) -> io::Result<()> {
    for path in instance_paths(instances_dir).await? {
        let Some(info) = read_instance_file(&path).await else {
            continue;
        };
        if !pid_alive(info.pid) {
            remove_if_present(&path).await?;
        }
    }
    Ok(())
}

async fn list_live_internal(instances_dir: &Path) -> io::Result<Vec<ServerInstanceInfo>> {
    let mut live = Vec::new();
    for path in instance_paths(instances_dir).await? {
        let Some(info) = read_instance_file(&path).await else {
            continue;
        };
        if pid_alive(info.pid) {
            live.push(info);
        } else {
            remove_if_present(&path).await?;
        }
    }
    live.sort_by_key(|info| info.started_at);
    Ok(live)
}

async fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    let result = unsafe {
        // SAFETY: kill(pid, 0) sends no signal and only probes a numeric PID.
        libc::kill(pid as libc::pid_t, 0)
    };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    let handle = unsafe {
        // SAFETY: OpenProcess is called with a numeric PID and no inherited
        // handle; the returned handle is closed below when non-null.
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
    };
    if handle.is_null() {
        return io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED as i32);
    }
    unsafe {
        // SAFETY: handle was returned by OpenProcess and has not been closed.
        CloseHandle(handle);
    }
    true
}

pub fn resolve_server_instances_dir(home_dir: Option<&Path>) -> PathBuf {
    match home_dir {
        Some(home_dir) => home_dir.join("server").join("instances"),
        None => {
            // MIGRATION-TODO:
            // Original: instanceRegistry.ts, resolveServerInstancesDir()
            // Missing dependency: agent-core-v2 resolveKimiHome().
            todo!("resolve default Kimi home through kimi-code-agent-core-v2")
        }
    }
}

pub async fn list_live_server_instances(
    home_dir: Option<&Path>,
) -> io::Result<Vec<ServerInstanceInfo>> {
    InstanceRegistry::create(InstanceRegistryOptions {
        instances_dir: Some(resolve_server_instances_dir(home_dir)),
        ..InstanceRegistryOptions::default()
    })
    .list_live()
    .await
}

pub async fn get_live_server_instance(
    home_dir: Option<&Path>,
) -> io::Result<Option<ServerInstanceInfo>> {
    Ok(list_live_server_instances(home_dir)
        .await?
        .into_iter()
        .next())
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    fn registry(directory: &Path, now: i64, interval: Duration) -> InstanceRegistry {
        InstanceRegistry::create(InstanceRegistryOptions {
            instances_dir: Some(directory.to_owned()),
            now: Arc::new(move || now),
            heartbeat_interval: interval,
        })
    }

    fn info(started_at: i64) -> RegistrationInfo {
        RegistrationInfo {
            pid: std::process::id(),
            host: "127.0.0.1".into(),
            port: 58_627,
            started_at,
            host_version: None,
        }
    }

    #[tokio::test]
    async fn registers_updates_lists_and_releases() {
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(directory.path(), 2_000, Duration::from_secs(60));
        let registration = registry.register(info(1_000)).await.unwrap();
        let path = directory
            .path()
            .join(format!("{}.json", registration.server_id));
        let disk: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
        assert_eq!(disk["server_id"], registration.server_id);
        assert_eq!(disk["heartbeat_at"], 2_000);
        assert_eq!(disk["port"], 58_627);

        registration.update(Some(58_628)).await.unwrap();
        assert_eq!(registry.list_live().await.unwrap()[0].port, 58_628);
        registration.release().await.unwrap();
        registration.release().await.unwrap();
        registration.update(Some(9_999)).await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn lists_by_start_time_and_sweeps_dead_processes() {
        let directory = tempfile::tempdir().unwrap();
        let registry = registry(directory.path(), 1, Duration::from_secs(60));
        let newer = registry.register(info(200)).await.unwrap();
        let older = registry.register(info(100)).await.unwrap();
        let dead_path = directory.path().join("dead.json");
        let dead = ServerInstanceDisk {
            server_id: "dead".into(),
            pid: i32::MAX as u32,
            host: "127.0.0.1".into(),
            port: 58_627,
            started_at: 1,
            heartbeat_at: 1,
            host_version: None,
        };
        fs::write(&dead_path, serde_json::to_vec(&dead).unwrap())
            .await
            .unwrap();

        let live = registry.list_live().await.unwrap();
        assert_eq!(
            live.iter()
                .map(|info| info.server_id.as_str())
                .collect::<Vec<_>>(),
            [older.server_id.as_str(), newer.server_id.as_str()]
        );
        assert!(!dead_path.exists());
        older.release().await.unwrap();
        newer.release().await.unwrap();
    }

    #[tokio::test]
    async fn heartbeats_stop_before_release_removes_file() {
        let directory = tempfile::tempdir().unwrap();
        let tick = Arc::new(AtomicI64::new(0));
        let now_tick = Arc::clone(&tick);
        let registry = InstanceRegistry::create(InstanceRegistryOptions {
            instances_dir: Some(directory.path().to_owned()),
            now: Arc::new(move || now_tick.fetch_add(1, Ordering::SeqCst)),
            heartbeat_interval: Duration::from_millis(1),
        });
        let registration = registry.register(info(100)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(tick.load(Ordering::SeqCst) > 1);
        let path = directory
            .path()
            .join(format!("{}.json", registration.server_id));
        registration.release().await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn convenience_readers_use_home_server_instances() {
        let home = tempfile::tempdir().unwrap();
        let instances_dir = resolve_server_instances_dir(Some(home.path()));
        let registry = registry(&instances_dir, 1, Duration::from_secs(60));
        let registration = registry.register(info(100)).await.unwrap();
        assert_eq!(
            get_live_server_instance(Some(home.path()))
                .await
                .unwrap()
                .unwrap()
                .server_id,
            registration.server_id
        );
        registration.release().await.unwrap();
    }
}
