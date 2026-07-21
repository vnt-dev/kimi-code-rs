use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Display, Formatter},
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, SystemTime},
};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::tui::commands::goal::MAX_GOAL_OBJECTIVE_LENGTH;

const GOAL_QUEUE_FILE: &str = "upcoming-goals.json";
const GOAL_QUEUE_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpcomingGoal {
    pub id: String,
    pub objective: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalQueueSnapshot {
    pub goals: Vec<UpcomingGoal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalQueueMoveDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalQueueSessionSummary {
    pub session_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalQueueSession {
    pub id: String,
    pub summary: Option<GoalQueueSessionSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalQueueErrorCode {
    ConfigInvalid,
    GoalObjectiveEmpty,
    GoalObjectiveTooLong,
    GoalNotFound,
}

impl GoalQueueErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfigInvalid => "config.invalid",
            Self::GoalObjectiveEmpty => "goal.objective_empty",
            Self::GoalObjectiveTooLong => "goal.objective_too_long",
            Self::GoalNotFound => "goal.not_found",
        }
    }
}

#[derive(Debug)]
pub enum GoalQueueError {
    Kimi {
        code: GoalQueueErrorCode,
        message: String,
    },
    MissingSessionDirectory {
        session_id: String,
    },
    Io(std::io::Error),
}

impl GoalQueueError {
    pub fn code(&self) -> Option<GoalQueueErrorCode> {
        match self {
            Self::Kimi { code, .. } => Some(*code),
            Self::MissingSessionDirectory { .. } | Self::Io(_) => None,
        }
    }
}

impl Display for GoalQueueError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kimi { message, .. } => formatter.write_str(message),
            Self::MissingSessionDirectory { session_id } => write!(
                formatter,
                "Session {session_id} does not expose a session directory"
            ),
            Self::Io(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for GoalQueueError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Kimi { .. } | Self::MissingSessionDirectory { .. } => None,
        }
    }
}

impl From<std::io::Error> for GoalQueueError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct GoalQueueFile {
    version: u8,
    goals: Vec<UpcomingGoal>,
}

type QueueLock = AsyncMutex<()>;
type QueueLockMap = HashMap<PathBuf, Weak<QueueLock>>;

static QUEUE_MUTATION_LOCKS: OnceLock<Mutex<QueueLockMap>> = OnceLock::new();

/// Original:
///   apps/kimi-code/src/tui/goal-queue-store.ts
///   readGoalQueue()
pub async fn read_goal_queue(
    session: &GoalQueueSession,
) -> Result<GoalQueueSnapshot, GoalQueueError> {
    let state = read_queue_file(session).await?;
    Ok(to_snapshot(state))
}

pub async fn append_goal_queue_item(
    session: &GoalQueueSession,
    objective: &str,
) -> Result<GoalQueueSnapshot, GoalQueueError> {
    let objective = normalize_objective(objective)?;
    with_queue_mutation_lock(session, async move {
        let state = read_queue_file(session).await?;
        let now = iso_timestamp(SystemTime::now());
        let goal = UpcomingGoal {
            id: Uuid::new_v4().to_string(),
            objective,
            created_at: now.clone(),
            updated_at: now,
        };
        let mut goals = state.goals;
        goals.push(goal);
        let next = GoalQueueFile {
            version: GOAL_QUEUE_VERSION,
            goals,
        };
        write_queue_file(session, &next).await?;
        Ok(to_snapshot(next))
    })
    .await
}

pub async fn update_goal_queue_item(
    session: &GoalQueueSession,
    goal_id: &str,
    objective: &str,
) -> Result<GoalQueueSnapshot, GoalQueueError> {
    let objective = normalize_objective(objective)?;
    with_queue_mutation_lock(session, async move {
        let mut state = read_queue_file(session).await?;
        let index = find_goal_index(&state, goal_id)?;
        let updated_at = timestamp_after(&state.goals[index].updated_at);
        state.goals[index].objective = objective;
        state.goals[index].updated_at = updated_at;
        write_queue_file(session, &state).await?;
        Ok(to_snapshot(state))
    })
    .await
}

pub async fn remove_goal_queue_item(
    session: &GoalQueueSession,
    goal_id: &str,
) -> Result<GoalQueueSnapshot, GoalQueueError> {
    with_queue_mutation_lock(session, async move {
        let mut state = read_queue_file(session).await?;
        let index = find_goal_index(&state, goal_id)?;
        state.goals.remove(index);
        write_queue_file(session, &state).await?;
        Ok(to_snapshot(state))
    })
    .await
}

pub async fn restore_goal_queue_item(
    session: &GoalQueueSession,
    goal: UpcomingGoal,
) -> Result<GoalQueueSnapshot, GoalQueueError> {
    with_queue_mutation_lock(session, async move {
        let mut state = read_queue_file(session).await?;
        if state.goals.iter().any(|item| item.id == goal.id) {
            return Ok(to_snapshot(state));
        }
        state.goals.insert(0, goal);
        write_queue_file(session, &state).await?;
        Ok(to_snapshot(state))
    })
    .await
}

pub async fn move_goal_queue_item(
    session: &GoalQueueSession,
    goal_id: &str,
    direction: GoalQueueMoveDirection,
) -> Result<GoalQueueSnapshot, GoalQueueError> {
    with_queue_mutation_lock(session, async move {
        let mut state = read_queue_file(session).await?;
        let index = find_goal_index(&state, goal_id)?;
        let target_index = match direction {
            GoalQueueMoveDirection::Up => index.checked_sub(1),
            GoalQueueMoveDirection::Down => index.checked_add(1),
        };
        let Some(target_index) = target_index.filter(|target| *target < state.goals.len()) else {
            return Ok(to_snapshot(state));
        };
        state.goals.swap(index, target_index);
        write_queue_file(session, &state).await?;
        Ok(to_snapshot(state))
    })
    .await
}

fn goal_queue_path(session: &GoalQueueSession) -> Result<PathBuf, GoalQueueError> {
    session
        .summary
        .as_ref()
        .and_then(|summary| summary.session_dir.as_ref())
        .filter(|directory| !directory.to_string_lossy().trim().is_empty())
        .map(|directory| directory.join(GOAL_QUEUE_FILE))
        .ok_or_else(|| GoalQueueError::MissingSessionDirectory {
            session_id: session.id.clone(),
        })
}

async fn read_queue_file(session: &GoalQueueSession) -> Result<GoalQueueFile, GoalQueueError> {
    let file_path = goal_queue_path(session)?;
    let raw = match tokio::fs::read_to_string(&file_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(empty_queue_file()),
        Err(error) => return Err(error.into()),
    };
    let parsed = serde_json::from_str::<Value>(&raw).map_err(|error| GoalQueueError::Kimi {
        code: GoalQueueErrorCode::ConfigInvalid,
        message: format!("Invalid JSON in goal queue: {error}"),
    })?;

    let Some(file) = parse_goal_queue_file(&parsed) else {
        let empty = empty_queue_file();
        write_queue_file(session, &empty).await?;
        return Ok(empty);
    };
    Ok(file)
}

async fn write_queue_file(
    session: &GoalQueueSession,
    file: &GoalQueueFile,
) -> Result<(), GoalQueueError> {
    let file_path = goal_queue_path(session)?;
    if let Some(parent) = file_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut encoded = serde_json::to_string_pretty(file).map_err(|error| GoalQueueError::Kimi {
        code: GoalQueueErrorCode::ConfigInvalid,
        message: error.to_string(),
    })?;
    encoded.push('\n');
    tokio::fs::write(file_path, encoded).await?;
    Ok(())
}

async fn with_queue_mutation_lock<T>(
    session: &GoalQueueSession,
    work: impl Future<Output = Result<T, GoalQueueError>>,
) -> Result<T, GoalQueueError> {
    let file_path = goal_queue_path(session)?;
    let lock = {
        let map = QUEUE_MUTATION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut locks = match map.lock() {
            Ok(locks) => locks,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(lock) = locks.get(&file_path).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(AsyncMutex::new(()));
            locks.insert(file_path, Arc::downgrade(&lock));
            lock
        }
    };
    let _guard = lock.lock().await;
    work.await
}

fn empty_queue_file() -> GoalQueueFile {
    GoalQueueFile {
        version: GOAL_QUEUE_VERSION,
        goals: Vec::new(),
    }
}

fn to_snapshot(file: GoalQueueFile) -> GoalQueueSnapshot {
    GoalQueueSnapshot { goals: file.goals }
}

fn normalize_objective(value: &str) -> Result<String, GoalQueueError> {
    let objective = value.trim().to_owned();
    if objective.is_empty() {
        return Err(GoalQueueError::Kimi {
            code: GoalQueueErrorCode::GoalObjectiveEmpty,
            message: "Goal objective cannot be empty".to_owned(),
        });
    }
    if objective.encode_utf16().count() > MAX_GOAL_OBJECTIVE_LENGTH {
        return Err(GoalQueueError::Kimi {
            code: GoalQueueErrorCode::GoalObjectiveTooLong,
            message: format!("Goal objective cannot exceed {MAX_GOAL_OBJECTIVE_LENGTH} characters"),
        });
    }
    Ok(objective)
}

fn find_goal_index(file: &GoalQueueFile, goal_id: &str) -> Result<usize, GoalQueueError> {
    file.goals
        .iter()
        .position(|goal| goal.id == goal_id)
        .ok_or_else(|| GoalQueueError::Kimi {
            code: GoalQueueErrorCode::GoalNotFound,
            message: "No queued goal found".to_owned(),
        })
}

fn parse_goal_queue_file(value: &Value) -> Option<GoalQueueFile> {
    let object = value.as_object()?;
    if object.get("version")?.as_f64()? != f64::from(GOAL_QUEUE_VERSION) {
        return None;
    }
    let goals = object
        .get("goals")?
        .as_array()?
        .iter()
        .map(parse_upcoming_goal)
        .collect::<Option<Vec<_>>>()?;
    Some(GoalQueueFile {
        version: GOAL_QUEUE_VERSION,
        goals,
    })
}

fn parse_upcoming_goal(value: &Value) -> Option<UpcomingGoal> {
    let object = value.as_object()?;
    Some(UpcomingGoal {
        id: non_empty_string(object.get("id")?)?,
        objective: non_empty_string(object.get("objective")?)?,
        created_at: non_empty_string(object.get("createdAt")?)?,
        updated_at: non_empty_string(object.get("updatedAt")?)?,
    })
}

fn non_empty_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn timestamp_after(previous: &str) -> String {
    let now = SystemTime::now();
    let previous = DateTime::parse_from_rfc3339(previous)
        .ok()
        .map(SystemTime::from);
    let timestamp = previous
        .filter(|previous| now <= *previous)
        .and_then(|previous| previous.checked_add(Duration::from_millis(1)))
        .unwrap_or(now);
    iso_timestamp(timestamp)
}

fn iso_timestamp(timestamp: SystemTime) -> String {
    DateTime::<Utc>::from(timestamp).to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn temp_session() -> GoalQueueSession {
        GoalQueueSession {
            id: "session_test".to_owned(),
            summary: Some(GoalQueueSessionSummary {
                session_dir: Some(
                    std::env::temp_dir().join(format!("kimi-goal-queue-{}", Uuid::new_v4())),
                ),
            }),
        }
    }

    fn session_dir(session: &GoalQueueSession) -> &Path {
        session
            .summary
            .as_ref()
            .and_then(|summary| summary.session_dir.as_deref())
            .unwrap_or_else(|| Path::new(""))
    }

    async fn clean(session: &GoalQueueSession) {
        let directory = session_dir(session);
        if directory.starts_with(std::env::temp_dir()) {
            let _ = tokio::fs::remove_dir_all(directory).await;
        }
    }

    #[tokio::test]
    async fn reads_missing_file_and_appends_trimmed_goal() {
        let session = temp_session();
        assert!(
            read_goal_queue(&session)
                .await
                .is_ok_and(|value| value.goals.is_empty())
        );

        let snapshot = append_goal_queue_item(&session, "  Ship release notes  ")
            .await
            .unwrap_or_else(|error| panic!("append failed: {error}"));
        assert_eq!(snapshot.goals.len(), 1);
        assert_eq!(snapshot.goals[0].objective, "Ship release notes");

        let raw = tokio::fs::read_to_string(session_dir(&session).join(GOAL_QUEUE_FILE))
            .await
            .unwrap_or_default();
        assert!(raw.ends_with('\n'));
        assert!(raw.contains("\"version\": 1"));
        clean(&session).await;
    }

    #[tokio::test]
    async fn preserves_concurrent_appends() {
        let session = temp_session();
        let mut tasks = Vec::new();
        for index in 0..10 {
            let session = session.clone();
            tasks.push(tokio::spawn(async move {
                append_goal_queue_item(&session, &format!("Queued goal {}", index + 1)).await
            }));
        }
        for task in tasks {
            assert!(task.await.is_ok_and(|result| result.is_ok()));
        }

        let snapshot = read_goal_queue(&session)
            .await
            .unwrap_or_else(|error| panic!("read failed: {error}"));
        assert_eq!(snapshot.goals.len(), 10);
        clean(&session).await;
    }

    #[tokio::test]
    async fn updates_removes_restores_and_moves_goals() {
        let session = temp_session();
        let first = append_goal_queue_item(&session, "First")
            .await
            .unwrap_or_else(|error| panic!("append failed: {error}"));
        let first_goal = first.goals[0].clone();
        append_goal_queue_item(&session, "Second")
            .await
            .unwrap_or_else(|error| panic!("append failed: {error}"));
        let third = append_goal_queue_item(&session, "Third")
            .await
            .unwrap_or_else(|error| panic!("append failed: {error}"));

        let updated = update_goal_queue_item(&session, &first_goal.id, "  Published  ")
            .await
            .unwrap_or_else(|error| panic!("update failed: {error}"));
        assert_eq!(updated.goals[0].objective, "Published");
        assert_ne!(updated.goals[0].updated_at, first_goal.updated_at);

        let moved = move_goal_queue_item(&session, &third.goals[2].id, GoalQueueMoveDirection::Up)
            .await
            .unwrap_or_else(|error| panic!("move failed: {error}"));
        assert_eq!(
            moved
                .goals
                .iter()
                .map(|goal| goal.objective.as_str())
                .collect::<Vec<_>>(),
            ["Published", "Third", "Second"]
        );

        let removed = remove_goal_queue_item(&session, &first_goal.id)
            .await
            .unwrap_or_else(|error| panic!("remove failed: {error}"));
        assert_eq!(removed.goals.len(), 2);
        let restored = restore_goal_queue_item(&session, first_goal.clone())
            .await
            .unwrap_or_else(|error| panic!("restore failed: {error}"));
        assert_eq!(restored.goals[0].id, first_goal.id);
        let deduped = restore_goal_queue_item(&session, first_goal)
            .await
            .unwrap_or_else(|error| panic!("restore failed: {error}"));
        assert_eq!(deduped.goals.len(), 3);
        clean(&session).await;
    }

    #[tokio::test]
    async fn rejects_invalid_objectives_and_missing_goals_with_codes() {
        let session = temp_session();
        let empty = append_goal_queue_item(&session, "  ").await;
        let long = append_goal_queue_item(&session, &"🦀".repeat(2_001)).await;
        let missing = remove_goal_queue_item(&session, "missing").await;

        assert_eq!(
            empty.err().and_then(|error| error.code()),
            Some(GoalQueueErrorCode::GoalObjectiveEmpty)
        );
        assert_eq!(
            long.err().and_then(|error| error.code()),
            Some(GoalQueueErrorCode::GoalObjectiveTooLong)
        );
        assert_eq!(
            missing.err().and_then(|error| error.code()),
            Some(GoalQueueErrorCode::GoalNotFound)
        );
        clean(&session).await;
    }

    #[tokio::test]
    async fn normalizes_structurally_invalid_files_but_preserves_invalid_json() {
        let session = temp_session();
        tokio::fs::create_dir_all(session_dir(&session))
            .await
            .unwrap_or_else(|error| panic!("mkdir failed: {error}"));
        let path = session_dir(&session).join(GOAL_QUEUE_FILE);
        tokio::fs::write(&path, r#"{"version":1,"goals":[{"bad":true}]}"#)
            .await
            .unwrap_or_else(|error| panic!("write failed: {error}"));

        assert!(
            read_goal_queue(&session)
                .await
                .is_ok_and(|snapshot| snapshot.goals.is_empty())
        );
        assert!(
            tokio::fs::read_to_string(&path)
                .await
                .is_ok_and(|raw| raw.contains("\"goals\": []"))
        );

        let partial = r#"{"version":1,"goals":["#;
        tokio::fs::write(&path, partial)
            .await
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        let error = read_goal_queue(&session).await.err();
        assert!(matches!(
            error,
            Some(GoalQueueError::Kimi {
                code: GoalQueueErrorCode::ConfigInvalid,
                ..
            })
        ));
        assert!(
            tokio::fs::read_to_string(&path)
                .await
                .is_ok_and(|raw| raw == partial)
        );
        clean(&session).await;
    }

    #[tokio::test]
    async fn rejects_a_session_without_a_directory() {
        let session = GoalQueueSession {
            id: "missing".to_owned(),
            summary: None,
        };

        assert_eq!(
            read_goal_queue(&session)
                .await
                .err()
                .map(|error| error.to_string()),
            Some("Session missing does not expose a session directory".to_owned())
        );
    }

    #[test]
    fn timestamp_is_strictly_after_a_future_or_equal_previous_value() {
        let future = SystemTime::now() + Duration::from_secs(60);
        let previous = iso_timestamp(future);
        let updated = DateTime::parse_from_rfc3339(&timestamp_after(&previous));
        let previous = DateTime::parse_from_rfc3339(&previous);

        assert!(matches!((updated, previous), (Ok(updated), Ok(previous)) if updated > previous));
    }
}
