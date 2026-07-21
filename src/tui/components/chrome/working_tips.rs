use std::{
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

const TIP_ROTATE_INTERVAL_MS: u128 = 10_000;
static RANDOM_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolbarTip {
    pub text: &'static str,
    pub solo: bool,
    pub priority: usize,
}

impl ToolbarTip {
    const fn new(text: &'static str, solo: bool, priority: usize) -> Self {
        Self {
            text,
            solo,
            priority,
        }
    }
}

/// Original: `src/tui/constant/tips.ts`, `WORKING_TIPS`.
pub const WORKING_TIPS: &[ToolbarTip] = &[
    ToolbarTip::new(
        "ctrl-s to add guidance without waiting for the turn to finish",
        true,
        2,
    ),
    ToolbarTip::new(
        "/tasks to check progress and status for background tasks",
        false,
        2,
    ),
    ToolbarTip::new("/init: generate AGENTS.md", false, 2),
    ToolbarTip::new("Try /dance for a hidden Easter egg", false, 1),
    ToolbarTip::new(
        "/plugins: manage plugins — try the \"superpowers\" plugin",
        true,
        3,
    ),
    ToolbarTip::new(
        "/plugins: manage plugins — try the \"Kimi Datasource\" for reliable financial, economic, and academic data",
        true,
        3,
    ),
    ToolbarTip::new(
        "ask Kimi to schedule tasks, e.g. \"remind me at 5pm\"",
        true,
        3,
    ),
    ToolbarTip::new("/sessions to browse and resume earlier sessions", true, 1),
    ToolbarTip::new(
        "/goal for multi-step work with a clear finish line",
        true,
        2,
    ),
    ToolbarTip::new(
        "/goal next to queue follow-up work while the current goal keeps running",
        true,
        1,
    ),
    ToolbarTip::new("/web: use the Web UI for a better experience", true, 1),
    ToolbarTip::new("@: mention files", false, 2),
    ToolbarTip::new("! to run a shell command", false, 2),
];

static WORKING_TIP_ROTATION: LazyLock<Vec<ToolbarTip>> =
    LazyLock::new(|| build_weighted_tips(WORKING_TIPS));

/// Smooth weighted round-robin used by both the footer and working-tip picker.
///
/// Original: `src/tui/components/chrome/footer.ts`, `buildWeightedTips()`.
pub fn build_weighted_tips(tips: &[ToolbarTip]) -> Vec<ToolbarTip> {
    #[derive(Clone, Copy)]
    struct WeightedTip {
        tip: ToolbarTip,
        weight: usize,
        current: isize,
    }

    let mut items = tips
        .iter()
        .copied()
        .map(|tip| WeightedTip {
            tip,
            weight: tip.priority.max(1),
            current: 0,
        })
        .collect::<Vec<_>>();
    let total = items.iter().map(|item| item.weight).sum::<usize>();
    let mut sequence = Vec::with_capacity(total);
    for _ in 0..total {
        let mut best_index = 0;
        for index in 0..items.len() {
            items[index].current += items[index].weight as isize;
            if items[index].current > items[best_index].current {
                best_index = index;
            }
        }
        items[best_index].current -= total as isize;
        sequence.push(items[best_index].tip);
    }
    sequence
}

// Original: working-tips.ts currentWorkingTip()
pub fn current_working_tip(now_ms: Option<u128>) -> Option<ToolbarTip> {
    if WORKING_TIP_ROTATION.is_empty() {
        return None;
    }
    let now_ms = now_ms.unwrap_or_else(system_time_ms);
    let index = (now_ms / TIP_ROTATE_INTERVAL_MS) as usize % WORKING_TIP_ROTATION.len();
    WORKING_TIP_ROTATION.get(index).copied()
}

// Original: working-tips.ts pickRandomWorkingTip()
pub fn pick_random_working_tip(exclude_text: Option<&str>) -> Option<ToolbarTip> {
    if WORKING_TIP_ROTATION.is_empty() {
        return None;
    }
    let candidates = WORKING_TIP_ROTATION
        .iter()
        .copied()
        .filter(|tip| {
            exclude_text.is_none()
                || WORKING_TIP_ROTATION.len() == 1
                || exclude_text != Some(tip.text)
        })
        .collect::<Vec<_>>();
    let pool = if candidates.is_empty() {
        WORKING_TIP_ROTATION.as_slice()
    } else {
        candidates.as_slice()
    };
    let seed = system_time_ms() as u64
        ^ RANDOM_SEQUENCE
            .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
            .rotate_left(17);
    pool.get((mix64(seed) as usize) % pool.len()).copied()
}

fn system_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_rotation_is_deterministic_and_respects_priority() {
        let tips = [
            ToolbarTip::new("one", false, 1),
            ToolbarTip::new("two", false, 3),
        ];
        let rotation = build_weighted_tips(&tips);
        assert_eq!(rotation.len(), 4);
        assert_eq!(rotation.iter().filter(|tip| tip.text == "one").count(), 1);
        assert_eq!(rotation.iter().filter(|tip| tip.text == "two").count(), 3);
        assert_eq!(rotation, build_weighted_tips(&tips));
    }

    #[test]
    fn current_tip_is_stable_for_a_rotation_interval() {
        let first = current_working_tip(Some(1_000_000)).expect("tip");
        let second = current_working_tip(Some(1_009_999)).expect("tip");
        assert_eq!(first, second);
        assert!(WORKING_TIPS.iter().any(|tip| tip.text == first.text));
    }

    #[test]
    fn random_tip_honors_exclusion_when_alternatives_exist() {
        let excluded = WORKING_TIPS[0].text;
        for _ in 0..50 {
            let tip = pick_random_working_tip(Some(excluded)).expect("tip");
            assert_ne!(tip.text, excluded);
            assert!(WORKING_TIPS.iter().any(|known| known.text == tip.text));
        }
    }
}
