//! Per-agent transcript subscription granularity.
//!
//! Original:
//!   `packages/transcript/src/granularity/grade.ts`

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptGrade {
    #[default]
    Off,
    Turn,
    Block,
    Delta,
}

pub const GRADE_RANK: [(TranscriptGrade, u8); 4] = [
    (TranscriptGrade::Off, 0),
    (TranscriptGrade::Turn, 1),
    (TranscriptGrade::Block, 2),
    (TranscriptGrade::Delta, 3),
];

impl TranscriptGrade {
    pub const fn rank(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Turn => 1,
            Self::Block => 2,
            Self::Delta => 3,
        }
    }
}

/// Per-session grade map. `None` values mirror optional Record entries.
pub type TranscriptGradeSpec = IndexMap<String, Option<TranscriptGrade>>;

pub fn grade_for(spec: Option<&TranscriptGradeSpec>, agent_id: &str) -> TranscriptGrade {
    let Some(spec) = spec else {
        return TranscriptGrade::Off;
    };
    spec.get(agent_id)
        .copied()
        .flatten()
        .or_else(|| spec.get("*").copied().flatten())
        .unwrap_or(TranscriptGrade::Off)
}

pub const fn needs_reset_on_transition(previous: TranscriptGrade, next: TranscriptGrade) -> bool {
    next.rank() > previous.rank()
}
