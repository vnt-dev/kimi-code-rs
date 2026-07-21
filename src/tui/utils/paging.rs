/// The derived view of a single page in a filtered list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageView {
    /// Zero-based index of the page containing the selected item.
    pub page: usize,
    /// Total number of pages. Empty lists still have one empty page.
    pub page_count: usize,
    /// Inclusive start index of the current page.
    pub start: usize,
    /// Exclusive end index of the current page, clamped to the item count.
    pub end: usize,
}

// Original:
//   apps/kimi-code/src/tui/utils/paging.ts
//   pageView()
//
// Rust adaptation:
//   Collection sizes are represented by `usize`. Signed arguments retain the
//   original clamping behavior for negative selections and non-positive sizes.
pub fn page_view(total: usize, selected_index: isize, page_size: isize) -> PageView {
    let size = usize::try_from(page_size).unwrap_or(1).max(1);
    let page_count = total.div_ceil(size).max(1);
    let safe_index = if total == 0 {
        0
    } else {
        usize::try_from(selected_index).unwrap_or(0).min(total - 1)
    };
    let page = (safe_index / size).min(page_count - 1);
    let start = page * size;
    let end = start.saturating_add(size).min(total);

    PageView {
        page,
        page_count,
        start,
        end,
    }
}

#[cfg(test)]
mod tests {
    use super::{PageView, page_view};

    #[test]
    fn keeps_the_selected_index_on_the_first_page() {
        assert_eq!(
            page_view(60, 3, 8),
            PageView {
                page: 0,
                page_count: 8,
                start: 0,
                end: 8,
            }
        );
    }

    #[test]
    fn derives_the_page_containing_the_selected_index() {
        assert_eq!(
            page_view(60, 12, 8),
            PageView {
                page: 1,
                page_count: 8,
                start: 8,
                end: 16,
            }
        );
    }

    #[test]
    fn clamps_the_final_page_slice_to_the_total() {
        assert_eq!(
            page_view(60, 59, 8),
            PageView {
                page: 7,
                page_count: 8,
                start: 56,
                end: 60,
            }
        );
    }

    #[test]
    fn clamps_a_selected_index_past_the_end_onto_the_last_page() {
        assert_eq!(
            page_view(10, 999, 4),
            PageView {
                page: 2,
                page_count: 3,
                start: 8,
                end: 10,
            }
        );
    }

    #[test]
    fn clamps_a_negative_selected_index_to_the_first_page() {
        assert_eq!(
            page_view(10, -5, 4),
            PageView {
                page: 0,
                page_count: 3,
                start: 0,
                end: 4,
            }
        );
    }

    #[test]
    fn returns_a_single_page_when_page_size_exceeds_the_total() {
        assert_eq!(
            page_view(5, 4, 8),
            PageView {
                page: 0,
                page_count: 1,
                start: 0,
                end: 5,
            }
        );
    }

    #[test]
    fn returns_a_single_empty_page_for_an_empty_list() {
        assert_eq!(
            page_view(0, 0, 8),
            PageView {
                page: 0,
                page_count: 1,
                start: 0,
                end: 0,
            }
        );
    }

    #[test]
    fn treats_a_non_positive_page_size_as_size_one() {
        assert_eq!(
            page_view(3, 2, 0),
            PageView {
                page: 2,
                page_count: 3,
                start: 2,
                end: 3,
            }
        );
        assert_eq!(page_view(3, 2, -10), page_view(3, 2, 1));
    }
}
