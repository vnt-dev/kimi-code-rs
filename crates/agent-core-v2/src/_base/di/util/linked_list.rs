use parking_lot::Mutex;
use std::sync::{Arc, Weak};

struct Node<T> {
    element: Option<T>,
    previous: Option<usize>,
    next: Option<usize>,
    generation: u64,
}

struct State<T> {
    nodes: Vec<Node<T>>,
    free: Vec<usize>,
    first: Option<usize>,
    last: Option<usize>,
    size: usize,
    next_generation: u64,
}

impl<T> Default for State<T> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            free: Vec::new(),
            first: None,
            last: None,
            size: 0,
            next_generation: 0,
        }
    }
}

pub struct LinkedList<T> {
    state: Arc<Mutex<State<T>>>,
}

impl<T> Default for LinkedList<T> {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
        }
    }
}

impl<T> LinkedList<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.state.lock().size
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // Original: packages/agent-core-v2/src/_base/di/util/linkedList.ts, LinkedList.push().
    pub fn push(&self, element: T) -> Removal<T> {
        let mut state = self.state.lock();
        let generation = state.next_generation;
        state.next_generation = state.next_generation.wrapping_add(1);
        let previous = state.last;
        let index = if let Some(index) = state.free.pop() {
            state.nodes[index] = Node {
                element: Some(element),
                previous,
                next: None,
                generation,
            };
            index
        } else {
            let index = state.nodes.len();
            state.nodes.push(Node {
                element: Some(element),
                previous,
                next: None,
                generation,
            });
            index
        };
        if let Some(previous) = previous {
            state.nodes[previous].next = Some(index);
        } else {
            state.first = Some(index);
        }
        state.last = Some(index);
        state.size += 1;
        Removal {
            state: Arc::downgrade(&self.state),
            index,
            generation,
        }
    }

    pub fn shift(&self) -> Option<T> {
        let mut state = self.state.lock();
        let index = state.first?;
        remove_node(&mut state, index)
    }

    pub fn snapshot(&self) -> Vec<T>
    where
        T: Clone,
    {
        let state = self.state.lock();
        let mut values = Vec::with_capacity(state.size);
        let mut current = state.first;
        while let Some(index) = current {
            let node = &state.nodes[index];
            if let Some(element) = &node.element {
                values.push(element.clone());
            }
            current = node.next;
        }
        values
    }
}

pub struct Removal<T> {
    state: Weak<Mutex<State<T>>>,
    index: usize,
    generation: u64,
}

impl<T> Removal<T> {
    pub fn remove(&self) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let mut state = state.lock();
        if state
            .nodes
            .get(self.index)
            .is_none_or(|node| node.element.is_none() || node.generation != self.generation)
        {
            return false;
        }
        let _ = remove_node(&mut state, self.index);
        true
    }
}

fn remove_node<T>(state: &mut State<T>, index: usize) -> Option<T> {
    let previous = state.nodes[index].previous;
    let next = state.nodes[index].next;
    match previous {
        Some(previous) => state.nodes[previous].next = next,
        None => state.first = next,
    }
    match next {
        Some(next) => state.nodes[next].previous = previous,
        None => state.last = previous,
    }
    let element = state.nodes[index].element.take();
    state.nodes[index].previous = None;
    state.nodes[index].next = None;
    if element.is_some() {
        state.size -= 1;
        state.free.push(index);
    }
    element
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_shift_and_middle_removal_preserve_order() {
        let list = LinkedList::new();
        let first = list.push(1);
        let middle = list.push(2);
        let last = list.push(3);
        assert_eq!(list.snapshot(), vec![1, 2, 3]);
        assert!(middle.remove());
        assert!(!middle.remove());
        assert_eq!(list.snapshot(), vec![1, 3]);
        assert_eq!(list.shift(), Some(1));
        assert!(!first.remove());
        assert!(last.remove());
        assert!(list.is_empty());
    }

    #[test]
    fn stale_removal_cannot_remove_a_reused_slot() {
        let list = LinkedList::new();
        let stale = list.push("old");
        assert_eq!(list.shift(), Some("old"));
        let current = list.push("new");
        assert!(!stale.remove());
        assert_eq!(list.snapshot(), vec!["new"]);
        assert!(current.remove());
    }
}
