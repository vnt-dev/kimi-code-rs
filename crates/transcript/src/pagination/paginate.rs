//! Cursor pagination over turn segments.
//!
//! Original:
//!   `packages/transcript/src/pagination/paginate.ts`

use crate::model::{TranscriptItem, TurnId, compare_turn_ids};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnPageQuery {
    pub before_turn: Option<TurnId>,
    pub after_turn: Option<TurnId>,
    pub page_size: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnPage {
    pub items: Vec<TranscriptItem>,
    pub has_more: bool,
}

pub fn paginate_turns(items: &[TranscriptItem], query: &TurnPageQuery) -> TurnPage {
    let page_size = query.page_size.max(1) as usize;
    let segments = split_segments(items);
    if segments.is_empty() {
        return TurnPage {
            items: Vec::new(),
            has_more: false,
        };
    }

    if let Some(after_turn) = &query.after_turn {
        let newer: Vec<_> = segments
            .into_iter()
            .filter(|segment| {
                segment
                    .turn_id
                    .is_some_and(|turn_id| compare_turn_ids(turn_id, after_turn).is_gt())
            })
            .collect();
        return page(&newer, page_size, Direction::Newer);
    }
    if let Some(before_turn) = &query.before_turn {
        let older: Vec<_> = segments
            .into_iter()
            .filter(|segment| {
                segment
                    .turn_id
                    .is_none_or(|turn_id| compare_turn_ids(turn_id, before_turn).is_lt())
            })
            .collect();
        return page(&older, page_size, Direction::Older);
    }
    page(&segments, page_size, Direction::Older)
}

struct Segment<'a> {
    items: Vec<&'a TranscriptItem>,
    turn_id: Option<&'a TurnId>,
}

fn split_segments(items: &[TranscriptItem]) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    let mut current_turn = None;

    for item in items {
        if let TranscriptItem::Turn(turn) = item {
            if !current.is_empty() {
                segments.push(Segment {
                    items: current,
                    turn_id: current_turn,
                });
                current = Vec::new();
            }
            current_turn = Some(&turn.turn_id);
        }
        current.push(item);
    }
    if !current.is_empty() {
        segments.push(Segment {
            items: current,
            turn_id: current_turn,
        });
    }
    segments
}

#[derive(Clone, Copy)]
enum Direction {
    Older,
    Newer,
}

fn page(segments: &[Segment<'_>], page_size: usize, direction: Direction) -> TurnPage {
    let head = segments.first().filter(|segment| segment.turn_id.is_none());
    let turn_segments = if head.is_some() {
        &segments[1..]
    } else {
        segments
    };

    match direction {
        Direction::Older => {
            let selected_start = turn_segments.len().saturating_sub(page_size);
            let selected = &turn_segments[selected_start..];
            let reaches_first_turn = selected.len() == turn_segments.len();
            let has_more = turn_segments.len() > selected.len() && !selected.is_empty();
            let mut items = Vec::new();
            if reaches_first_turn && let Some(head) = head {
                extend_segment(&mut items, head);
            }
            for segment in selected {
                extend_segment(&mut items, segment);
            }
            TurnPage { items, has_more }
        }
        Direction::Newer => {
            let selected_len = turn_segments.len().min(page_size);
            let selected = &turn_segments[..selected_len];
            let has_more = turn_segments.len() > selected.len() && !selected.is_empty();
            let mut items = Vec::new();
            for segment in selected {
                extend_segment(&mut items, segment);
            }
            TurnPage { items, has_more }
        }
    }
}

fn extend_segment(items: &mut Vec<TranscriptItem>, segment: &Segment<'_>) {
    items.extend(segment.items.iter().map(|item| (*item).clone()));
}

#[cfg(test)]
mod tests {
    use crate::model::{
        MarkerId, TranscriptMarker, TranscriptTurn, TurnOrigin, TurnState, item_id,
    };

    use super::*;

    fn marker(id: &str) -> TranscriptItem {
        TranscriptItem::Marker(TranscriptMarker {
            marker_id: MarkerId::from(id),
            marker: "skill".to_owned(),
            payload: None,
            at: None,
        })
    }

    fn turn(ordinal: i64) -> TranscriptItem {
        TranscriptItem::Turn(TranscriptTurn {
            turn_id: TurnId::new(format!("t{ordinal}")),
            ordinal,
            state: TurnState::Completed,
            origin: TurnOrigin::other(),
            prompt: None,
            attachment_ids: None,
            steps: Vec::new(),
            started_at: None,
            ended_at: None,
            usage: None,
        })
    }

    fn timeline() -> Vec<TranscriptItem> {
        let mut items = vec![marker("m0")];
        for ordinal in 1..=5 {
            items.push(turn(ordinal));
            items.push(marker(&format!("m{ordinal}")));
        }
        items
    }

    fn labels(page: &TurnPage) -> Vec<&str> {
        page.items.iter().map(item_id).collect()
    }

    #[test]
    fn paginates_newest_older_newer_head_and_marker_only_boundaries() {
        let items = timeline();
        let cases = [
            (
                TurnPageQuery {
                    before_turn: None,
                    after_turn: None,
                    page_size: 2,
                },
                vec!["t4", "m4", "t5", "m5"],
                true,
            ),
            (
                TurnPageQuery {
                    before_turn: Some(TurnId::from("t4")),
                    after_turn: None,
                    page_size: 2,
                },
                vec!["t2", "m2", "t3", "m3"],
                true,
            ),
            (
                TurnPageQuery {
                    before_turn: Some(TurnId::from("t2")),
                    after_turn: None,
                    page_size: 5,
                },
                vec!["m0", "t1", "m1"],
                false,
            ),
            (
                TurnPageQuery {
                    before_turn: None,
                    after_turn: Some(TurnId::from("t3")),
                    page_size: 2,
                },
                vec!["t4", "m4", "t5", "m5"],
                false,
            ),
            (
                TurnPageQuery {
                    before_turn: None,
                    after_turn: None,
                    page_size: 5,
                },
                vec![
                    "m0", "t1", "m1", "t2", "m2", "t3", "m3", "t4", "m4", "t5", "m5",
                ],
                false,
            ),
        ];
        for (query, expected, has_more) in cases {
            let page = paginate_turns(&items, &query);
            assert_eq!(labels(&page), expected);
            assert_eq!(page.has_more, has_more);
        }

        let marker_only = vec![marker("m0")];
        let page = paginate_turns(
            &marker_only,
            &TurnPageQuery {
                before_turn: None,
                after_turn: None,
                page_size: 3,
            },
        );
        assert_eq!(labels(&page), ["m0"]);
        assert!(!page.has_more);
    }
}
