//! Pure copy-on-write reducer for one agent transcript.
//!
//! Original:
//!   `packages/transcript/src/ops/apply.ts`
//!
//! Rust adaptation:
//!   Aggregate branches are shared through `Arc` and cloned only when an
//!   operation changes that branch. JavaScript reference-equality checks on
//!   payload arrays/objects are represented by value equality in Rust; state
//!   transitions, ordering, gaps, and accepted/no-op behavior otherwise
//!   follow the original reducer.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use indexmap::{IndexMap, IndexSet};

use crate::model::{
    AttachmentId, InteractionId, InteractionState, ModesMeta, StepId, StepState, TaskId, TaskKind,
    TaskState, TodoId, TranscriptAttachment, TranscriptFrame, TranscriptInteraction,
    TranscriptItem, TranscriptMeta, TranscriptMetaMerge, TranscriptStep, TranscriptTask,
    TranscriptTodo, TranscriptTurn, TurnId, TurnOrigin, TurnState, item_id, turn_ordinal,
};

use super::{AgentTranscriptSnapshot, AppendTarget, StepHeader, TranscriptOperation, TurnHeader};

/// Immutable aggregate state behind one `AgentTranscript`.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentState {
    pub items: Arc<Vec<TranscriptItem>>,
    pub tasks: Arc<IndexMap<TaskId, TranscriptTask>>,
    pub interactions: Arc<IndexMap<InteractionId, TranscriptInteraction>>,
    pub attachments: Arc<IndexMap<AttachmentId, TranscriptAttachment>>,
    pub todos: Arc<IndexMap<TodoId, TranscriptTodo>>,
    pub meta: Arc<TranscriptMeta>,
    pub pending_interactions: Arc<IndexSet<InteractionId>>,
    pub has_more_older: bool,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            items: Arc::new(Vec::new()),
            tasks: Arc::new(IndexMap::new()),
            interactions: Arc::new(IndexMap::new()),
            attachments: Arc::new(IndexMap::new()),
            todos: Arc::new(IndexMap::new()),
            meta: Arc::new(TranscriptMeta::default()),
            pending_interactions: Arc::new(IndexSet::new()),
            has_more_older: false,
        }
    }
}

/// Shared empty state counterpart of the TypeScript `EMPTY_AGENT_STATE`.
pub static EMPTY_AGENT_STATE: LazyLock<AgentState> = LazyLock::new(AgentState::default);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OffsetGap {
    pub expected: u64,
    pub got: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ApplyResult {
    pub state: AgentState,
    pub changed: bool,
    pub gap: Option<OffsetGap>,
}

impl ApplyResult {
    fn unchanged(state: &AgentState) -> Self {
        Self {
            state: state.clone(),
            changed: false,
            gap: None,
        }
    }

    fn changed(state: AgentState) -> Self {
        Self {
            state,
            changed: true,
            gap: None,
        }
    }

    fn gap(state: &AgentState, gap: OffsetGap) -> Self {
        Self {
            state: state.clone(),
            changed: false,
            gap: Some(gap),
        }
    }
}

/// Apply one L2 operation through the package's single convergence path.
pub fn apply_operation(state: &AgentState, operation: &TranscriptOperation) -> ApplyResult {
    match operation {
        TranscriptOperation::Reset { snapshot, .. } => apply_reset(snapshot),
        TranscriptOperation::TurnUpsert { turn } => apply_turn_upsert(state, turn),
        TranscriptOperation::StepUpsert { turn_id, step } => {
            apply_step_upsert(state, turn_id, step)
        }
        TranscriptOperation::FrameUpsert {
            turn_id,
            step_id,
            frame,
        } => apply_frame_upsert(state, turn_id, step_id, frame),
        TranscriptOperation::Append {
            target,
            offset,
            text,
        } => apply_append(state, target, *offset, text),
        TranscriptOperation::MarkerUpsert { item, before_turn } => apply_item_upsert(
            state,
            TranscriptItem::Marker(item.clone()),
            item.marker_id.as_ref(),
            *before_turn,
        ),
        TranscriptOperation::TaskRefUpsert { item, before_turn } => apply_item_upsert(
            state,
            TranscriptItem::TaskRef(item.clone()),
            item.ref_id.as_ref(),
            *before_turn,
        ),
        TranscriptOperation::TaskUpsert { task } => apply_task_upsert(state, task),
        TranscriptOperation::InteractionUpsert { interaction } => {
            apply_interaction_upsert(state, interaction)
        }
        TranscriptOperation::AttachmentUpsert { attachment } => {
            apply_attachment_upsert(state, attachment)
        }
        TranscriptOperation::TodoUpsert { todo } => apply_todo_upsert(state, todo),
        TranscriptOperation::MetaMerge { meta } => apply_meta_merge(state, meta),
        TranscriptOperation::ItemsRemove { ids } => apply_items_remove(state, ids),
    }
}

fn apply_reset(snapshot: &AgentTranscriptSnapshot) -> ApplyResult {
    // Pending derives from both the authoritative global entities and the
    // legacy inline interaction frames.
    let mut pending = IndexSet::new();
    for interaction in &snapshot.interactions {
        if interaction.state == InteractionState::Pending {
            pending.insert(interaction.interaction_id.clone());
        }
    }
    for item in &snapshot.items {
        let TranscriptItem::Turn(turn) = item else {
            continue;
        };
        for step in &turn.steps {
            for frame in &step.frames {
                if let TranscriptFrame::Interaction(frame) = frame
                    && frame.state == InteractionState::Pending
                {
                    pending.insert(frame.interaction_id.clone());
                }
            }
        }
    }

    let mut tasks = IndexMap::new();
    for task in &snapshot.tasks {
        tasks.insert(task.task_id.clone(), task.clone());
    }
    let mut interactions = IndexMap::new();
    for interaction in &snapshot.interactions {
        interactions.insert(interaction.interaction_id.clone(), interaction.clone());
    }
    let mut attachments = IndexMap::new();
    for attachment in &snapshot.attachments {
        attachments.insert(attachment.attachment_id.clone(), attachment.clone());
    }
    let mut todos = IndexMap::new();
    for todo in &snapshot.todos {
        todos.insert(todo.todo_id.clone(), todo.clone());
    }

    ApplyResult::changed(AgentState {
        items: Arc::new(snapshot.items.clone()),
        tasks: Arc::new(tasks),
        interactions: Arc::new(interactions),
        attachments: Arc::new(attachments),
        todos: Arc::new(todos),
        meta: Arc::new(snapshot.meta.clone()),
        pending_interactions: Arc::new(pending),
        has_more_older: snapshot.has_more_older.unwrap_or(false),
    })
}

fn skeleton_turn(turn_id: &TurnId) -> TranscriptTurn {
    TranscriptTurn {
        turn_id: turn_id.clone(),
        ordinal: turn_ordinal(turn_id) as i64,
        state: TurnState::Running,
        origin: TurnOrigin::other(),
        prompt: None,
        attachment_ids: None,
        steps: Vec::new(),
        started_at: None,
        ended_at: None,
        usage: None,
    }
}

fn skeleton_step(step_id: &StepId, turn_id: &TurnId) -> TranscriptStep {
    let ordinal = step_id
        .as_ref()
        .strip_prefix(turn_id.as_ref())
        .and_then(|suffix| suffix.strip_prefix('.'))
        .and_then(|suffix| suffix.parse::<i64>().ok())
        .unwrap_or(0);
    TranscriptStep {
        step_id: step_id.clone(),
        turn_id: turn_id.clone(),
        ordinal,
        state: StepState::Running,
        frames: Vec::new(),
        started_at: None,
        ended_at: None,
    }
}

fn get_turn<'a>(state: &'a AgentState, turn_id: &TurnId) -> Option<&'a TranscriptTurn> {
    state.items.iter().find_map(|item| match item {
        TranscriptItem::Turn(turn) if turn.turn_id == *turn_id => Some(turn),
        _ => None,
    })
}

fn insert_turn(items: &[TranscriptItem], turn: TranscriptTurn) -> Vec<TranscriptItem> {
    let at = items
        .iter()
        .position(|item| {
            matches!(item, TranscriptItem::Turn(existing) if existing.ordinal > turn.ordinal)
        })
        .unwrap_or(items.len());
    let mut next = items.to_vec();
    next.insert(at, TranscriptItem::Turn(turn));
    next
}

fn replace_turn(
    items: &[TranscriptItem],
    turn_id: &TurnId,
    replacement: TranscriptTurn,
) -> Vec<TranscriptItem> {
    items
        .iter()
        .map(|item| match item {
            TranscriptItem::Turn(turn) if turn.turn_id == *turn_id => {
                TranscriptItem::Turn(replacement.clone())
            }
            _ => item.clone(),
        })
        .collect()
}

fn apply_turn_upsert(state: &AgentState, header: &TurnHeader) -> ApplyResult {
    if let Some(existing) = get_turn(state, &header.turn_id) {
        if turn_equals(existing, header) {
            return ApplyResult::unchanged(state);
        }
        let replacement = header.clone().into_turn(existing.steps.clone());
        let mut next = state.clone();
        next.items = Arc::new(replace_turn(&state.items, &header.turn_id, replacement));
        return ApplyResult::changed(next);
    }

    let mut next = state.clone();
    next.items = Arc::new(insert_turn(
        &state.items,
        header.clone().into_turn(Vec::new()),
    ));
    ApplyResult::changed(next)
}

fn turn_equals(turn: &TranscriptTurn, header: &TurnHeader) -> bool {
    turn.ordinal == header.ordinal
        && turn.state == header.state
        && turn.prompt == header.prompt
        && turn.attachment_ids == header.attachment_ids
        && turn.started_at == header.started_at
        && turn.ended_at == header.ended_at
        && origin_kind_and_payload_equal(&turn.origin, &header.origin)
        && turn.usage == header.usage
}

// The source deliberately ignores cron/task `taskId` here and compares only
// `kind` plus the open `payload`; preserve that quirk.
fn origin_kind_and_payload_equal(left: &TurnOrigin, right: &TurnOrigin) -> bool {
    fn payload(origin: &TurnOrigin) -> &crate::model::OptionalJsonValue {
        match origin {
            TurnOrigin::User { payload }
            | TurnOrigin::Cron { payload, .. }
            | TurnOrigin::Task { payload, .. }
            | TurnOrigin::Hook { payload }
            | TurnOrigin::Compaction { payload }
            | TurnOrigin::Side { payload }
            | TurnOrigin::Other { payload } => payload,
        }
    }
    left.kind() == right.kind() && payload(left) == payload(right)
}

fn apply_step_upsert(state: &AgentState, turn_id: &TurnId, header: &StepHeader) -> ApplyResult {
    let existing_turn = get_turn(state, turn_id);
    let mut turn = existing_turn
        .cloned()
        .unwrap_or_else(|| skeleton_turn(turn_id));

    if let Some(index) = turn
        .steps
        .iter()
        .position(|step| step.step_id == header.step_id)
    {
        if step_equals(&turn.steps[index], header) {
            return ApplyResult::unchanged(state);
        }
        let frames = std::mem::take(&mut turn.steps[index].frames);
        turn.steps[index] = header.clone().into_step(frames);
    } else {
        turn.steps.push(header.clone().into_step(Vec::new()));
        turn.steps.sort_by_key(|step| step.ordinal);
    }

    let items = if existing_turn.is_some() {
        replace_turn(&state.items, turn_id, turn)
    } else {
        insert_turn(&state.items, turn)
    };
    let mut next = state.clone();
    next.items = Arc::new(items);
    ApplyResult::changed(next)
}

fn step_equals(step: &TranscriptStep, header: &StepHeader) -> bool {
    step.ordinal == header.ordinal
        && step.state == header.state
        && step.started_at == header.started_at
        && step.ended_at == header.ended_at
}

fn apply_frame_upsert(
    state: &AgentState,
    turn_id: &TurnId,
    step_id: &StepId,
    frame: &TranscriptFrame,
) -> ApplyResult {
    let existing_turn = get_turn(state, turn_id);
    let mut turn = existing_turn
        .cloned()
        .unwrap_or_else(|| skeleton_turn(turn_id));
    let existing_step = turn.steps.iter().position(|step| step.step_id == *step_id);
    let mut step = existing_step
        .map(|index| turn.steps[index].clone())
        .unwrap_or_else(|| skeleton_step(step_id, turn_id));

    if let Some(index) = step
        .frames
        .iter()
        .position(|candidate| candidate.frame_id() == frame.frame_id())
    {
        if frame_equals(&step.frames[index], frame) {
            return ApplyResult::unchanged(state);
        }
        step.frames[index] = frame.clone();
    } else {
        step.frames.push(frame.clone());
    }

    if let Some(index) = existing_step {
        turn.steps[index] = step;
    } else {
        turn.steps.push(step);
        turn.steps.sort_by_key(|entry| entry.ordinal);
    }

    let items = if existing_turn.is_some() {
        replace_turn(&state.items, turn_id, turn)
    } else {
        insert_turn(&state.items, turn)
    };
    let mut next = state.clone();
    next.items = Arc::new(items);
    if let Some(pending) = track_pending(&state.pending_interactions, frame) {
        next.pending_interactions = Arc::new(pending);
    }
    ApplyResult::changed(next)
}

fn frame_equals(left: &TranscriptFrame, right: &TranscriptFrame) -> bool {
    match (left, right) {
        (TranscriptFrame::Text(left), TranscriptFrame::Text(right)) => {
            left.text == right.text
                && left.role == right.role
                && left.attachment_ids == right.attachment_ids
                && left.task_id == right.task_id
        }
        (TranscriptFrame::Thinking(left), TranscriptFrame::Thinking(right)) => {
            left.text == right.text
        }
        (TranscriptFrame::Tool(left), TranscriptFrame::Tool(right)) => {
            left.state == right.state
                && left.tool_call_id == right.tool_call_id
                && left.name == right.name
                && left.view == right.view
                && left.input == right.input
                && left.output == right.output
                && left.display == right.display
                && left.error == right.error
                && left.task_id == right.task_id
                && left.approval_id == right.approval_id
                && left.todo_id == right.todo_id
                && left.agent_refs == right.agent_refs
        }
        (TranscriptFrame::Interaction(left), TranscriptFrame::Interaction(right)) => {
            left.state == right.state
                && left.request == right.request
                && left.response == right.response
        }
        (TranscriptFrame::Notice(left), TranscriptFrame::Notice(right)) => {
            left.message == right.message
                && left.level == right.level
                && left.detail == right.detail
        }
        _ => false,
    }
}

fn track_pending(
    pending: &IndexSet<InteractionId>,
    frame: &TranscriptFrame,
) -> Option<IndexSet<InteractionId>> {
    let TranscriptFrame::Interaction(frame) = frame else {
        return None;
    };
    if frame.state == InteractionState::Pending {
        if pending.contains(&frame.interaction_id) {
            return None;
        }
        let mut next = pending.clone();
        next.insert(frame.interaction_id.clone());
        return Some(next);
    }
    if !pending.contains(&frame.interaction_id) {
        return None;
    }
    let mut next = pending.clone();
    next.shift_remove(&frame.interaction_id);
    Some(next)
}

fn apply_append(state: &AgentState, target: &AppendTarget, offset: u64, text: &str) -> ApplyResult {
    match target {
        AppendTarget::Task { task_id } => apply_task_append(state, task_id, offset, text),
        AppendTarget::Frame {
            turn_id,
            step_id,
            frame_id,
        } => apply_frame_append(state, turn_id, step_id, frame_id, offset, text),
    }
}

fn apply_frame_append(
    state: &AgentState,
    turn_id: &TurnId,
    step_id: &StepId,
    frame_id: &crate::model::FrameId,
    offset: u64,
    text: &str,
) -> ApplyResult {
    let Some(turn) = get_turn(state, turn_id) else {
        return ApplyResult::gap(
            state,
            OffsetGap {
                expected: 0,
                got: offset,
            },
        );
    };
    let Some(step) = turn.steps.iter().find(|step| step.step_id == *step_id) else {
        return ApplyResult::gap(
            state,
            OffsetGap {
                expected: 0,
                got: offset,
            },
        );
    };
    let Some(frame) = step
        .frames
        .iter()
        .find(|frame| frame.frame_id() == frame_id)
    else {
        return ApplyResult::gap(
            state,
            OffsetGap {
                expected: 0,
                got: offset,
            },
        );
    };
    let current = match frame {
        TranscriptFrame::Text(frame) => &frame.text,
        TranscriptFrame::Thinking(frame) => &frame.text,
        _ => {
            return ApplyResult::gap(
                state,
                OffsetGap {
                    expected: 0,
                    got: offset,
                },
            );
        }
    };
    let merged = append_at_offset(current, offset, text);
    if let Some(gap) = merged.gap {
        return ApplyResult::gap(state, gap);
    }
    if !merged.changed {
        return ApplyResult::unchanged(state);
    }

    let mut next_frame = frame.clone();
    match &mut next_frame {
        TranscriptFrame::Text(frame) => frame.text = merged.text,
        TranscriptFrame::Thinking(frame) => frame.text = merged.text,
        _ => unreachable!("frame kind was checked above"),
    }
    let mut next_step = step.clone();
    for candidate in &mut next_step.frames {
        if candidate.frame_id() == frame_id {
            *candidate = next_frame.clone();
        }
    }
    let mut next_turn = turn.clone();
    for candidate in &mut next_turn.steps {
        if candidate.step_id == *step_id {
            *candidate = next_step.clone();
        }
    }
    let mut next = state.clone();
    next.items = Arc::new(replace_turn(&state.items, turn_id, next_turn));
    ApplyResult::changed(next)
}

fn apply_task_append(state: &AgentState, task_id: &TaskId, offset: u64, text: &str) -> ApplyResult {
    let current = state
        .tasks
        .get(task_id)
        .map_or("", |task| task.output_tail.as_str());
    let merged = append_at_offset(current, offset, text);
    if let Some(gap) = merged.gap {
        return ApplyResult::gap(state, gap);
    }
    if !merged.changed {
        return ApplyResult::unchanged(state);
    }

    let task = state
        .tasks
        .get(task_id)
        .cloned()
        .map(|mut task| {
            task.output_tail = merged.text.clone();
            task
        })
        .unwrap_or_else(|| TranscriptTask {
            task_id: task_id.clone(),
            kind: TaskKind::Other,
            state: TaskState::Running,
            detached: false,
            description: None,
            agent_id: None,
            output_tail: merged.text,
            started_at: None,
            ended_at: None,
        });
    let mut tasks = state.tasks.as_ref().clone();
    tasks.insert(task_id.clone(), task);
    let mut next = state.clone();
    next.tasks = Arc::new(tasks);
    ApplyResult::changed(next)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendAtOffsetResult {
    pub text: String,
    pub changed: bool,
    pub gap: Option<OffsetGap>,
}

/// Place a chunk using JavaScript UTF-16 code-unit offsets.
pub fn append_at_offset(local: &str, offset: u64, chunk: &str) -> AppendAtOffsetResult {
    let local_units: Vec<u16> = local.encode_utf16().collect();
    let chunk_units: Vec<u16> = chunk.encode_utf16().collect();
    let expected = local_units.len() as u64;
    let Ok(offset_index) = usize::try_from(offset) else {
        return append_gap(local, expected, offset);
    };
    if offset_index > local_units.len() {
        return append_gap(local, expected, offset);
    }

    let duplicate_end = offset_index.saturating_add(chunk_units.len());
    if duplicate_end <= local_units.len() && local_units[offset_index..duplicate_end] == chunk_units
    {
        return AppendAtOffsetResult {
            text: local.to_owned(),
            changed: false,
            gap: None,
        };
    }

    let overlap = local_units.len() - offset_index;
    if local_units[offset_index..] != chunk_units[..chunk_units.len().min(overlap)]
        || overlap > chunk_units.len()
    {
        return append_gap(local, expected, offset);
    }
    let novel = &chunk_units[overlap..];
    if novel.is_empty() {
        return AppendAtOffsetResult {
            text: local.to_owned(),
            changed: false,
            gap: None,
        };
    }

    let mut merged = local_units[..offset_index].to_vec();
    merged.extend_from_slice(&chunk_units);
    let Ok(text) = String::from_utf16(&merged) else {
        // Rust strings cannot represent the unpaired-surrogate result that a
        // JavaScript slice could create. Treat that placement as divergence.
        return append_gap(local, expected, offset);
    };
    AppendAtOffsetResult {
        text,
        changed: true,
        gap: None,
    }
}

fn append_gap(local: &str, expected: u64, got: u64) -> AppendAtOffsetResult {
    AppendAtOffsetResult {
        text: local.to_owned(),
        changed: false,
        gap: Some(OffsetGap { expected, got }),
    }
}

fn apply_item_upsert(
    state: &AgentState,
    item: TranscriptItem,
    id: &str,
    before_turn: Option<i64>,
) -> ApplyResult {
    if state.items.iter().any(|entry| item_id(entry) == id) {
        let mut changed = false;
        let items = state
            .items
            .iter()
            .map(|entry| {
                if item_id(entry) != id || entry == &item {
                    return entry.clone();
                }
                changed = true;
                item.clone()
            })
            .collect();
        if !changed {
            return ApplyResult::unchanged(state);
        }
        let mut next = state.clone();
        next.items = Arc::new(items);
        return ApplyResult::changed(next);
    }

    let mut items = state.items.as_ref().clone();
    if let Some(anchor) = before_turn {
        let at = items
            .iter()
            .position(|entry| matches!(entry, TranscriptItem::Turn(turn) if turn.ordinal >= anchor))
            .unwrap_or(items.len());
        items.insert(at, item);
    } else {
        items.push(item);
    }
    let mut next = state.clone();
    next.items = Arc::new(items);
    ApplyResult::changed(next)
}

fn apply_items_remove(state: &AgentState, ids: &[String]) -> ApplyResult {
    let drop: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let removed_turns: Vec<&TranscriptTurn> = state
        .items
        .iter()
        .filter_map(|item| match item {
            TranscriptItem::Turn(turn) if drop.contains(turn.turn_id.as_ref()) => Some(turn),
            _ => None,
        })
        .collect();
    let items: Vec<_> = state
        .items
        .iter()
        .filter(|item| !drop.contains(item_id(item)))
        .cloned()
        .collect();
    if items.len() == state.items.len() {
        return ApplyResult::unchanged(state);
    }

    let mut next = state.clone();
    next.items = Arc::new(items);
    if !removed_turns.is_empty() {
        let mut anchored_tool_call_ids = HashSet::new();
        let mut pending = state.pending_interactions.as_ref().clone();
        for turn in removed_turns {
            for step in &turn.steps {
                for frame in &step.frames {
                    match frame {
                        TranscriptFrame::Tool(frame) => {
                            anchored_tool_call_ids.insert(frame.tool_call_id.as_str());
                        }
                        TranscriptFrame::Interaction(frame) => {
                            pending.shift_remove(&frame.interaction_id);
                        }
                        _ => {}
                    }
                }
            }
        }

        let dead_entity_ids: Vec<_> = state
            .interactions
            .values()
            .filter(|interaction| {
                anchored_tool_call_ids.contains(interaction.tool_call_id.as_str())
            })
            .map(|interaction| interaction.interaction_id.clone())
            .collect();
        if !dead_entity_ids.is_empty() {
            let mut interactions = state.interactions.as_ref().clone();
            for id in dead_entity_ids {
                interactions.shift_remove(&id);
                pending.shift_remove(&id);
            }
            next.interactions = Arc::new(interactions);
        }
        next.pending_interactions = Arc::new(pending);
    }
    ApplyResult::changed(next)
}

fn apply_task_upsert(state: &AgentState, task: &TranscriptTask) -> ApplyResult {
    if state
        .tasks
        .get(&task.task_id)
        .is_some_and(|current| task_equals(current, task))
    {
        return ApplyResult::unchanged(state);
    }
    let mut tasks = state.tasks.as_ref().clone();
    tasks.insert(task.task_id.clone(), task.clone());
    let mut next = state.clone();
    next.tasks = Arc::new(tasks);
    ApplyResult::changed(next)
}

fn task_equals(left: &TranscriptTask, right: &TranscriptTask) -> bool {
    left.kind == right.kind
        && left.state == right.state
        && left.detached == right.detached
        && left.description == right.description
        && left.agent_id == right.agent_id
        && left.output_tail == right.output_tail
        && left.started_at == right.started_at
        && left.ended_at == right.ended_at
}

fn apply_interaction_upsert(
    state: &AgentState,
    interaction: &TranscriptInteraction,
) -> ApplyResult {
    if state
        .interactions
        .get(&interaction.interaction_id)
        .is_some_and(|current| interaction_equals(current, interaction))
    {
        return ApplyResult::unchanged(state);
    }
    let mut interactions = state.interactions.as_ref().clone();
    interactions.insert(interaction.interaction_id.clone(), interaction.clone());
    let mut next = state.clone();
    next.interactions = Arc::new(interactions);

    let mut pending = state.pending_interactions.as_ref().clone();
    let pending_changed = if interaction.state == InteractionState::Pending {
        pending.insert(interaction.interaction_id.clone())
    } else {
        pending.shift_remove(&interaction.interaction_id)
    };
    if pending_changed {
        next.pending_interactions = Arc::new(pending);
    }
    ApplyResult::changed(next)
}

fn interaction_equals(left: &TranscriptInteraction, right: &TranscriptInteraction) -> bool {
    left.interaction_kind == right.interaction_kind
        && left.tool_call_id == right.tool_call_id
        && left.state == right.state
        && left.request == right.request
        && left.response == right.response
}

fn apply_attachment_upsert(state: &AgentState, attachment: &TranscriptAttachment) -> ApplyResult {
    if state
        .attachments
        .get(&attachment.attachment_id)
        .is_some_and(|current| attachment_equals(current, attachment))
    {
        return ApplyResult::unchanged(state);
    }
    let mut attachments = state.attachments.as_ref().clone();
    attachments.insert(attachment.attachment_id.clone(), attachment.clone());
    let mut next = state.clone();
    next.attachments = Arc::new(attachments);
    ApplyResult::changed(next)
}

fn attachment_equals(left: &TranscriptAttachment, right: &TranscriptAttachment) -> bool {
    left.media_type == right.media_type
        && left.name == right.name
        && left.size == right.size
        && left.source == right.source
        && left.placeholder == right.placeholder
}

fn apply_todo_upsert(state: &AgentState, todo: &TranscriptTodo) -> ApplyResult {
    if state
        .todos
        .get(&todo.todo_id)
        .is_some_and(|current| current.items == todo.items && current.updated_at == todo.updated_at)
    {
        return ApplyResult::unchanged(state);
    }
    let mut todos = state.todos.as_ref().clone();
    todos.insert(todo.todo_id.clone(), todo.clone());
    let mut next = state.clone();
    next.todos = Arc::new(todos);
    ApplyResult::changed(next)
}

fn apply_meta_merge(state: &AgentState, merge: &TranscriptMetaMerge) -> ApplyResult {
    let modes = merge.modes.as_ref().map_or_else(
        || state.meta.modes.clone(),
        |merge_modes| {
            let plan = match &merge_modes.plan {
                None => state
                    .meta
                    .modes
                    .as_ref()
                    .and_then(|modes| modes.plan.clone()),
                Some(value) => value.clone(),
            };
            let swarm = match &merge_modes.swarm {
                None => state
                    .meta
                    .modes
                    .as_ref()
                    .and_then(|modes| modes.swarm.clone()),
                Some(value) => value.clone(),
            };
            if plan.is_none() && swarm.is_none() {
                None
            } else {
                Some(ModesMeta { plan, swarm })
            }
        },
    );
    let next_meta = TranscriptMeta {
        goal: merge.goal.clone().or_else(|| state.meta.goal.clone()),
        modes,
        activity: merge.activity.or(state.meta.activity),
    };

    // `meta.modes` creates a fresh JS object whenever at least one badge
    // remains, so such merges notify even if their values are identical.
    let modes_recreated = merge.modes.is_some() && next_meta.modes.is_some();
    if !modes_recreated && next_meta == *state.meta {
        return ApplyResult::unchanged(state);
    }
    let mut next = state.clone();
    next.meta = Arc::new(next_meta);
    ApplyResult::changed(next)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::{
        FrameId, InteractionFrame, InteractionKind, MarkerId, ModesMetaMerge, PlanMode, TaskRefId,
        ThinkingFrame, ToolCallFrame, ToolFrameState, TranscriptMarker, TranscriptTaskRef,
    };

    fn turn_header(id: &str, ordinal: i64) -> TurnHeader {
        TurnHeader {
            turn_id: TurnId::from(id),
            ordinal,
            state: TurnState::Running,
            origin: TurnOrigin::User { payload: None },
            prompt: None,
            attachment_ids: None,
            started_at: None,
            ended_at: None,
            usage: None,
        }
    }

    fn thinking(id: &str, text: &str) -> TranscriptFrame {
        TranscriptFrame::Thinking(ThinkingFrame {
            frame_id: FrameId::from(id),
            text: text.to_owned(),
        })
    }

    #[test]
    fn reduces_missing_parents_and_preserves_prior_snapshot() {
        let initial = AgentState::default();
        let frame = TranscriptOperation::FrameUpsert {
            turn_id: TurnId::from("t9"),
            step_id: StepId::from("t9.2"),
            frame: thinking("t9.2.f1", "x"),
        };
        let first = apply_operation(&initial, &frame);
        assert!(first.changed);
        let old_items = first.state.items.clone();

        let step = TranscriptOperation::StepUpsert {
            turn_id: TurnId::from("t9"),
            step: StepHeader {
                step_id: StepId::from("t9.1"),
                turn_id: TurnId::from("t9"),
                ordinal: 1,
                state: StepState::Completed,
                started_at: None,
                ended_at: None,
            },
        };
        let second = apply_operation(&first.state, &step);
        let TranscriptItem::Turn(turn) = &second.state.items[0] else {
            panic!("expected turn");
        };
        assert_eq!(turn.steps[0].step_id.as_ref(), "t9.1");
        assert_eq!(turn.steps[1].step_id.as_ref(), "t9.2");
        let TranscriptItem::Turn(old_turn) = &old_items[0] else {
            panic!("expected prior turn");
        };
        assert_eq!(old_turn.steps.len(), 1);
        assert!(!apply_operation(&first.state, &frame).changed);
    }

    #[test]
    fn append_offsets_follow_utf16_duplicate_overlap_and_gap_rules() {
        let cases = [
            ("abc", 3, "d", "abcd", true, None),
            ("abc", 1, "bc", "abc", false, None),
            ("abc", 1, "bcd", "abcd", true, None),
            (
                "abc",
                5,
                "x",
                "abc",
                false,
                Some(OffsetGap {
                    expected: 3,
                    got: 5,
                }),
            ),
            (
                "hello",
                2,
                " world",
                "hello",
                false,
                Some(OffsetGap {
                    expected: 5,
                    got: 2,
                }),
            ),
            ("hello wo", 6, "world", "hello world", true, None),
            ("你", 1, "好", "你好", true, None),
            ("😀", 2, "!", "😀!", true, None),
        ];
        for (local, offset, chunk, text, changed, gap) in cases {
            assert_eq!(
                append_at_offset(local, offset, chunk),
                AppendAtOffsetResult {
                    text: text.to_owned(),
                    changed,
                    gap
                }
            );
        }
        assert_eq!(
            append_at_offset("😀", 1, "!").gap,
            Some(OffsetGap {
                expected: 2,
                got: 1
            })
        );
    }

    #[test]
    fn tracks_pending_and_removes_tool_anchored_interactions() {
        let mut state = apply_operation(
            &AgentState::default(),
            &TranscriptOperation::TurnUpsert {
                turn: turn_header("t1", 1),
            },
        )
        .state;
        state = apply_operation(
            &state,
            &TranscriptOperation::FrameUpsert {
                turn_id: TurnId::from("t1"),
                step_id: StepId::from("t1.1"),
                frame: TranscriptFrame::Tool(Box::new(ToolCallFrame {
                    frame_id: FrameId::from("tool-1"),
                    tool_call_id: "call-1".to_owned(),
                    name: "Bash".to_owned(),
                    view: None,
                    state: ToolFrameState::Running,
                    input: None,
                    output: None,
                    display: None,
                    error: None,
                    task_id: None,
                    approval_id: None,
                    todo_id: None,
                    agent_refs: None,
                })),
            },
        )
        .state;
        state = apply_operation(
            &state,
            &TranscriptOperation::InteractionUpsert {
                interaction: TranscriptInteraction {
                    interaction_id: InteractionId::from("approval-1"),
                    interaction_kind: InteractionKind::Approval,
                    tool_call_id: "call-1".to_owned(),
                    state: InteractionState::Pending,
                    request: None,
                    response: None,
                },
            },
        )
        .state;
        assert_eq!(
            state.pending_interactions.iter().next().unwrap().as_ref(),
            "approval-1"
        );

        let removed = apply_operation(
            &state,
            &TranscriptOperation::ItemsRemove {
                ids: vec!["t1".to_owned()],
            },
        );
        assert!(removed.state.items.is_empty());
        assert!(removed.state.interactions.is_empty());
        assert!(removed.state.pending_interactions.is_empty());
    }

    #[test]
    fn merges_and_clears_modes_with_absent_keys_preserved() {
        let initial = TranscriptOperation::MetaMerge {
            meta: TranscriptMetaMerge {
                modes: Some(ModesMetaMerge {
                    plan: Some(Some(PlanMode {
                        review_path: Some("/plan".to_owned()),
                    })),
                    swarm: Some(Some(Default::default())),
                }),
                ..Default::default()
            },
        };
        let state = apply_operation(&AgentState::default(), &initial).state;
        let clear_plan = TranscriptOperation::MetaMerge {
            meta: TranscriptMetaMerge {
                modes: Some(ModesMetaMerge {
                    plan: Some(None),
                    swarm: None,
                }),
                ..Default::default()
            },
        };
        let state = apply_operation(&state, &clear_plan).state;
        assert!(state.meta.modes.as_ref().unwrap().plan.is_none());
        assert!(state.meta.modes.as_ref().unwrap().swarm.is_some());

        let clear_swarm = TranscriptOperation::MetaMerge {
            meta: TranscriptMetaMerge {
                modes: Some(ModesMetaMerge {
                    plan: None,
                    swarm: Some(None),
                }),
                ..Default::default()
            },
        };
        let result = apply_operation(&state, &clear_swarm);
        assert!(result.state.meta.modes.is_none());
    }

    #[test]
    fn anchored_items_keep_turn_relative_order_and_replace_in_place() {
        let mut state = AgentState::default();
        for (id, ordinal) in [("t2", 2), ("t0", 0), ("t1", 1)] {
            state = apply_operation(
                &state,
                &TranscriptOperation::TurnUpsert {
                    turn: turn_header(id, ordinal),
                },
            )
            .state;
        }
        state = apply_operation(
            &state,
            &TranscriptOperation::MarkerUpsert {
                item: TranscriptMarker {
                    marker_id: MarkerId::from("m1"),
                    marker: "skill".to_owned(),
                    payload: None,
                    at: None,
                },
                before_turn: Some(1),
            },
        )
        .state;
        state = apply_operation(
            &state,
            &TranscriptOperation::TaskRefUpsert {
                item: TranscriptTaskRef {
                    ref_id: TaskRefId::from("r1"),
                    task_id: TaskId::from("task-1"),
                    at: None,
                },
                before_turn: Some(2),
            },
        )
        .state;
        let labels: Vec<_> = state.items.iter().map(item_id).collect();
        assert_eq!(labels, ["t0", "m1", "t1", "r1", "t2"]);

        let replaced = apply_operation(
            &state,
            &TranscriptOperation::MarkerUpsert {
                item: TranscriptMarker {
                    marker_id: MarkerId::from("m1"),
                    marker: "skill".to_owned(),
                    payload: Some(Some(json!({"v": 1}))),
                    at: None,
                },
                before_turn: Some(0),
            },
        );
        assert_eq!(
            replaced.state.items.iter().map(item_id).collect::<Vec<_>>(),
            labels
        );
    }

    #[test]
    fn reset_derives_pending_from_global_and_legacy_channels() {
        let legacy = TranscriptFrame::Interaction(InteractionFrame {
            frame_id: FrameId::from("f1"),
            interaction_id: InteractionId::from("legacy"),
            interaction_kind: InteractionKind::Question,
            tool_call_id: None,
            state: InteractionState::Pending,
            request: None,
            response: None,
        });
        let snapshot = AgentTranscriptSnapshot {
            items: vec![TranscriptItem::Turn(TranscriptTurn {
                steps: vec![TranscriptStep {
                    step_id: StepId::from("t1.0"),
                    turn_id: TurnId::from("t1"),
                    ordinal: 0,
                    state: StepState::Running,
                    frames: vec![legacy],
                    started_at: None,
                    ended_at: None,
                }],
                ..turn_header("t1", 1).into_turn(Vec::new())
            })],
            interactions: vec![TranscriptInteraction {
                interaction_id: InteractionId::from("global"),
                interaction_kind: InteractionKind::Approval,
                tool_call_id: "call".to_owned(),
                state: InteractionState::Pending,
                request: None,
                response: None,
            }],
            ..Default::default()
        };
        let result = apply_operation(
            &AgentState::default(),
            &TranscriptOperation::Reset {
                agent_id: crate::model::AgentId::from("main"),
                snapshot,
            },
        );
        assert_eq!(
            result
                .state
                .pending_interactions
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>(),
            ["global", "legacy"]
        );
    }
}
