//! Agent-level and session-level transcript stores.
//!
//! Original modules: `packages/transcript/src/store/*`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::IndexMap;

pub mod agent_transcript;
pub mod transcript_store;

pub use agent_transcript::*;
pub use transcript_store::*;

type Listener<T> = Rc<RefCell<Box<dyn FnMut(&T)>>>;
type ListenerMap<T> = Rc<RefCell<IndexMap<u64, Listener<T>>>>;

struct ListenerRegistry<T> {
    listeners: ListenerMap<T>,
    next_id: Cell<u64>,
}

impl<T: 'static> ListenerRegistry<T> {
    fn new() -> Self {
        Self {
            listeners: Rc::new(RefCell::new(IndexMap::new())),
            next_id: Cell::new(0),
        }
    }

    fn register(&self, listener: impl FnMut(&T) + 'static) -> Disposable {
        let id = self.next_id.get();
        self.next_id.set(id.saturating_add(1));
        self.listeners
            .borrow_mut()
            .insert(id, Rc::new(RefCell::new(Box::new(listener))));
        let listeners: Weak<RefCell<IndexMap<u64, Listener<T>>>> = Rc::downgrade(&self.listeners);
        Disposable::new(move || {
            if let Some(listeners) = listeners.upgrade() {
                listeners.borrow_mut().shift_remove(&id);
            }
        })
    }

    fn emit(&self, event: &T) {
        // Iterate by monotonic id so disposing a later callback during an
        // earlier callback prevents delivery, matching JavaScript Set
        // iteration without holding a RefCell borrow across user code.
        let mut after = None;
        loop {
            let next = self
                .listeners
                .borrow()
                .keys()
                .copied()
                .find(|id| after.is_none_or(|previous| *id > previous));
            let Some(id) = next else {
                break;
            };
            after = Some(id);
            let listener = self.listeners.borrow().get(&id).cloned();
            if let Some(listener) = listener {
                (listener.borrow_mut())(event);
            }
        }
    }
}

/// Explicit listener-disposal handle.
///
/// Dropping the handle intentionally does not unregister the callback: the
/// TypeScript source also keeps a listener until `dispose()` is invoked.
pub struct Disposable {
    dispose: Option<Box<dyn FnOnce()>>,
}

impl Disposable {
    fn new(dispose: impl FnOnce() + 'static) -> Self {
        Self {
            dispose: Some(Box::new(dispose)),
        }
    }

    pub fn dispose(&mut self) {
        if let Some(dispose) = self.dispose.take() {
            dispose();
        }
    }
}
