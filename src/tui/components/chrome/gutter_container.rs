use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole},
    utils::render_cache::is_render_cache_enabled,
};

#[derive(Clone)]
struct ChildRenderCache {
    identity: usize,
    rendered: Vec<String>,
    prefixed: Vec<String>,
}

#[derive(Clone)]
struct GutterRenderCache {
    width: usize,
    children: Vec<ChildRenderCache>,
    output: Vec<String>,
}

/// Container that reserves logical left/right gutters around child content.
///
/// Original: `src/tui/components/chrome/gutter-container.ts`.
pub struct GutterContainer {
    left_pad: usize,
    right_pad: usize,
    children: Vec<Box<dyn Component>>,
    render_cache: Option<GutterRenderCache>,
}

impl GutterContainer {
    pub fn new(left_pad: usize, right_pad: usize) -> Self {
        Self {
            left_pad,
            right_pad,
            children: Vec::new(),
            render_cache: None,
        }
    }

    pub fn add_child(&mut self, child: impl Component + 'static) {
        self.children.push(Box::new(child));
    }

    pub fn add_boxed_child(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }

    pub fn remove_child_at(&mut self, index: usize) -> Option<Box<dyn Component>> {
        (index < self.children.len()).then(|| self.children.remove(index))
    }

    pub fn replace_child_at(
        &mut self,
        index: usize,
        child: Box<dyn Component>,
    ) -> Option<Box<dyn Component>> {
        if index >= self.children.len() {
            return None;
        }
        Some(std::mem::replace(&mut self.children[index], child))
    }

    pub fn children(&self) -> &[Box<dyn Component>] {
        &self.children
    }

    fn render_children(&mut self, width: usize) -> Vec<String> {
        let inner_width = width
            .saturating_sub(self.left_pad.saturating_add(self.right_pad))
            .max(1);
        let leading = " ".repeat(self.left_pad);
        let cache_valid = is_render_cache_enabled()
            && self.render_cache.as_ref().is_some_and(|cache| {
                cache.width == width && cache.children.len() == self.children.len()
            });
        let mut child_caches = Vec::with_capacity(self.children.len());
        let mut all_reused = cache_valid;
        for (index, child) in self.children.iter_mut().enumerate() {
            let identity = (&**child as *const dyn Component as *const ()) as usize;
            let rendered = child.render(inner_width);
            let cached = cache_valid
                .then(|| self.render_cache.as_ref()?.children.get(index))
                .flatten();
            let prefixed = if cached
                .is_some_and(|cached| cached.identity == identity && cached.rendered == rendered)
            {
                cached.expect("checked cache").prefixed.clone()
            } else {
                all_reused = false;
                rendered
                    .iter()
                    .map(|line| format!("{leading}{line}"))
                    .collect()
            };
            child_caches.push(ChildRenderCache {
                identity,
                rendered,
                prefixed,
            });
        }
        let output = if all_reused {
            self.render_cache
                .as_ref()
                .expect("valid cache")
                .output
                .clone()
        } else {
            child_caches
                .iter()
                .flat_map(|child| child.prefixed.iter().cloned())
                .collect()
        };
        if is_render_cache_enabled() {
            self.render_cache = Some(GutterRenderCache {
                width,
                children: child_caches,
                output: output.clone(),
            });
        }
        output
    }
}

impl Component for GutterContainer {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_children(width)
    }

    fn invalidate(&mut self) {
        self.render_cache = None;
        for child in &mut self.children {
            child.invalidate();
        }
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct FakeChild {
        seen_widths: Arc<Mutex<Vec<usize>>>,
        lines: Vec<String>,
    }

    impl Component for FakeChild {
        fn render(&mut self, width: usize) -> Vec<String> {
            self.seen_widths.lock().expect("widths").push(width);
            self.lines.clone()
        }

        fn invalidate(&mut self) {}

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn child(lines: &[&str]) -> (FakeChild, Arc<Mutex<Vec<usize>>>) {
        let widths = Arc::new(Mutex::new(Vec::new()));
        (
            FakeChild {
                seen_widths: Arc::clone(&widths),
                lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            },
            widths,
        )
    }

    #[test]
    fn prefixes_lines_and_passes_reduced_width() {
        let (child, widths) = child(&["hello", "world"]);
        let mut container = GutterContainer::new(2, 3);
        container.add_child(child);
        assert_eq!(container.render(20), ["  hello", "  world"]);
        assert_eq!(*widths.lock().expect("widths"), [15]);
    }

    #[test]
    fn clamps_inner_width_and_stacks_children() {
        let (first, first_widths) = child(&["a1", "a2"]);
        let (second, second_widths) = child(&["b1"]);
        let mut container = GutterContainer::new(5, 5);
        container.add_child(first);
        container.add_child(second);
        assert_eq!(container.render(2), ["     a1", "     a2", "     b1"]);
        assert_eq!(*first_widths.lock().expect("widths"), [1]);
        assert_eq!(*second_widths.lock().expect("widths"), [1]);
    }

    #[test]
    fn preserves_ansi_and_empty_container_output() {
        let mut empty = GutterContainer::new(2, 2);
        assert!(empty.render(20).is_empty());
        let (child, _) = child(&["\u{1b}[31mred\u{1b}[0m"]);
        empty.add_child(child);
        assert_eq!(empty.render(20), ["  \u{1b}[31mred\u{1b}[0m"]);
    }

    #[test]
    fn detects_append_remove_and_replacement_without_global_invalidation() {
        let (first, _) = child(&["a"]);
        let (second, _) = child(&["b"]);
        let (replacement, _) = child(&["c"]);
        let mut container = GutterContainer::new(1, 0);
        container.add_child(first);
        assert_eq!(container.render(10), [" a"]);
        container.add_child(second);
        assert_eq!(container.render(10), [" a", " b"]);
        let removed = container.remove_child_at(0);
        assert!(removed.is_some());
        assert_eq!(container.render(10), [" b"]);
        let old = container.replace_child_at(0, Box::new(replacement));
        assert!(old.is_some());
        assert_eq!(container.render(10), [" c"]);
    }
}
