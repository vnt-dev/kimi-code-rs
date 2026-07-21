use crate::tui::{
    fuzzy::fuzzy_filter,
    keys::{ListKey, matches_list_key},
};

use super::{
    paging::{PageView, page_view},
    printable_key::{is_printable_char, printable_char},
};

const DEFAULT_PAGE_SIZE: isize = 8;

pub struct SearchableList<T> {
    items: Vec<T>,
    to_search_text: Box<dyn Fn(&T) -> String + Send + Sync>,
    page_size: isize,
    searchable: bool,
    query: String,
    cursor: isize,
}

#[derive(Debug, PartialEq)]
pub struct SearchableListView<'a, T> {
    pub items: Vec<&'a T>,
    pub page: PageView,
    pub selected_index: usize,
    pub query: &'a str,
}

impl<T> SearchableList<T> {
    // Original:
    //   apps/kimi-code/src/tui/utils/searchable-list.ts
    //   SearchableList.constructor()
    pub fn new<F>(
        items: Vec<T>,
        to_search_text: F,
        page_size: Option<isize>,
        initial_index: Option<isize>,
        searchable: bool,
    ) -> Self
    where
        F: Fn(&T) -> String + Send + Sync + 'static,
    {
        Self {
            items,
            to_search_text: Box::new(to_search_text),
            page_size: page_size.unwrap_or(DEFAULT_PAGE_SIZE),
            searchable,
            query: String::new(),
            cursor: initial_index.unwrap_or(0).max(0),
        }
    }

    // Original: SearchableList.filtered()
    pub fn filtered(&self) -> Vec<&T> {
        if self.query.is_empty() {
            self.items.iter().collect()
        } else {
            fuzzy_filter(&self.items, &self.query, &self.to_search_text)
        }
    }

    // Original: SearchableList.selected()
    pub fn selected(&self) -> Option<&T> {
        let items = self.filtered();
        let last = items.len().checked_sub(1)?;
        items
            .get(usize::try_from(self.cursor).unwrap_or(0).min(last))
            .copied()
    }

    // Original: SearchableList.view()
    pub fn view(&self) -> SearchableListView<'_, T> {
        let items = self.filtered();
        let selected_index = usize::try_from(self.cursor)
            .unwrap_or(0)
            .min(items.len().saturating_sub(1));
        SearchableListView {
            page: page_view(items.len(), self.cursor, self.page_size),
            items,
            selected_index,
            query: &self.query,
        }
    }

    // Original: SearchableList.moveUp()
    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1).max(0);
    }

    // Original: SearchableList.moveDown()
    pub fn move_down(&mut self) {
        let last = isize::try_from(self.filtered().len().saturating_sub(1)).unwrap_or(isize::MAX);
        self.cursor = self.cursor.saturating_add(1).min(last.max(0));
    }

    // Original: SearchableList.pageUp()
    pub fn page_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(self.page_size).max(0);
    }

    // Original: SearchableList.pageDown()
    pub fn page_down(&mut self) {
        let last = isize::try_from(self.filtered().len().saturating_sub(1)).unwrap_or(isize::MAX);
        self.cursor = self.cursor.saturating_add(self.page_size).min(last.max(0));
    }

    // Original: SearchableList.clearQuery()
    pub fn clear_query(&mut self) -> bool {
        if self.query.is_empty() {
            return false;
        }
        self.query.clear();
        self.cursor = 0;
        true
    }

    // Original: SearchableList.handleKey()
    pub fn handle_key(&mut self, data: &str) -> bool {
        if matches_list_key(data, ListKey::Up) {
            self.move_up();
            return true;
        }
        if matches_list_key(data, ListKey::Down) {
            self.move_down();
            return true;
        }
        if matches_list_key(data, ListKey::PageUp) {
            self.page_up();
            return true;
        }
        if matches_list_key(data, ListKey::PageDown) {
            self.page_down();
            return true;
        }
        if !self.searchable {
            return false;
        }
        if matches_list_key(data, ListKey::Backspace) {
            if !self.query.is_empty() {
                self.query.pop();
                self.cursor = 0;
            }
            return true;
        }
        let character = printable_char(data);
        if is_printable_char(&character) {
            self.query.push_str(&character);
            self.cursor = 0;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::SearchableList;

    fn list(searchable: bool, initial_index: isize) -> SearchableList<String> {
        SearchableList::new(
            (0..10).map(|index| format!("item{index:02}")).collect(),
            Clone::clone,
            Some(4),
            Some(initial_index),
            searchable,
        )
    }

    #[test]
    fn derives_page_math_and_pages_by_page_size() {
        let mut list = list(false, 0);
        assert_eq!(list.view().page.page_count, 3);
        assert_eq!((list.view().page.start, list.view().page.end), (0, 4));
        list.page_down();
        assert_eq!(list.view().selected_index, 4);
        assert_eq!(list.view().page.page, 1);
        list.page_up();
        assert_eq!(list.view().page.page, 0);
    }

    #[test]
    fn clamps_cursor_at_both_ends_and_selects_the_item() {
        let mut list = list(false, 2);
        assert_eq!(list.selected().map(String::as_str), Some("item02"));
        for _ in 0..20 {
            list.move_down();
        }
        assert_eq!(list.view().selected_index, 9);
        list.page_down();
        assert_eq!(list.view().selected_index, 9);
        for _ in 0..20 {
            list.move_up();
        }
        assert_eq!(list.view().selected_index, 0);
    }

    #[test]
    fn filters_resets_cursor_and_clears_query() {
        let mut list = list(true, 5);
        for character in "item09".chars() {
            assert!(list.handle_key(&character.to_string()));
        }
        assert_eq!(list.view().query, "item09");
        assert!(
            list.view()
                .items
                .iter()
                .any(|item| item.as_str() == "item09")
        );
        assert!(
            !list
                .view()
                .items
                .iter()
                .any(|item| item.as_str() == "item00")
        );
        assert_eq!(list.view().selected_index, 0);
        assert_eq!(list.selected().map(String::as_str), Some("item09"));
        assert!(list.clear_query());
        assert_eq!(list.view().items.len(), 10);
        assert!(!list.clear_query());
    }

    #[test]
    fn trims_query_on_backspace() {
        let mut list = list(true, 0);
        for character in "item0".chars() {
            list.handle_key(&character.to_string());
        }
        assert!(list.handle_key("\u{7f}"));
        assert_eq!(list.view().query, "item");
    }

    #[test]
    fn navigation_is_always_consumed_but_search_editing_is_optional() {
        let mut navigation = list(false, 0);
        for key in ["\u{1b}[A", "\u{1b}[B", "\u{1b}[5~", "\u{1b}[6~"] {
            assert!(navigation.handle_key(key));
        }
        assert!(!navigation.handle_key("a"));
        assert!(!navigation.handle_key("\u{7f}"));
        assert_eq!(navigation.view().query, "");

        let mut search = list(true, 0);
        assert!(search.handle_key("\u{1b}[97u"));
        assert_eq!(search.view().query, "a");
        assert!(search.handle_key("\u{7f}"));
        assert_eq!(search.view().query, "");
    }
}
