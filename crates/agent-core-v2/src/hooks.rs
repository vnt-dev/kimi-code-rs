//! Ordered chain-of-responsibility hook slots.
//!
//! Original: `packages/agent-core-v2/src/hooks.ts`.
//!
//! Rust adaptation: handlers pass the active mutable context explicitly to
//! `next`. Passing a different mutable value provides the original override
//! behavior while avoiding aliased mutable borrows across `await`.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use futures_util::future::BoxFuture;

use crate::_base::{
    di::lifecycle::{DisposableHandle, to_disposable},
    lifecycle::lifecycle_machine::BoxError,
};

pub type HookResult = Result<(), BoxError>;
pub type HookNext<C> =
    Arc<dyn for<'a> Fn(&'a mut C) -> BoxFuture<'a, HookResult> + Send + Sync + 'static>;
pub type HookHandler<C> = Arc<
    dyn for<'a> Fn(&'a mut C, HookNext<C>) -> BoxFuture<'a, HookResult> + Send + Sync + 'static,
>;
pub type HookTerminal<C> =
    Arc<dyn for<'a> Fn(&'a mut C) -> BoxFuture<'a, HookResult> + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HookRegisterOptions<'a> {
    pub before: Option<&'a str>,
    pub after: Option<&'a str>,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum HookRegistrationError {
    #[error("Hook registration cannot specify both before and after")]
    ConflictingPosition,

    #[error("Hook target \"{0}\" is not registered")]
    TargetNotRegistered(String),
}

struct HookEntry<C> {
    token: u64,
    id: String,
    handler: HookHandler<C>,
}

impl<C> Clone for HookEntry<C> {
    fn clone(&self) -> Self {
        Self {
            token: self.token,
            id: self.id.clone(),
            handler: Arc::clone(&self.handler),
        }
    }
}

pub struct OrderedHookSlot<C> {
    entries: Arc<Mutex<Vec<HookEntry<C>>>>,
    next_token: AtomicU64,
}

impl<C> Default for OrderedHookSlot<C> {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            next_token: AtomicU64::new(1),
        }
    }
}

impl<C> OrderedHookSlot<C>
where
    C: Send + 'static,
{
    pub fn new() -> Self {
        Self::default()
    }

    // Original: OrderedHookSlot.register().
    pub fn register(
        &self,
        id: impl Into<String>,
        handler: HookHandler<C>,
        options: HookRegisterOptions<'_>,
    ) -> Result<DisposableHandle, HookRegistrationError> {
        if options.before.is_some() && options.after.is_some() {
            return Err(HookRegistrationError::ConflictingPosition);
        }
        let id = id.into();
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let entry = HookEntry {
            token,
            id: id.clone(),
            handler,
        };
        {
            let mut entries = self.entries.lock().unwrap();
            if let Some(index) = entries.iter().position(|entry| entry.id == id) {
                entries.remove(index);
            }
            let target = options.before.or(options.after);
            let insert_at = match target {
                None => entries.len(),
                Some(target) => {
                    let index = entries
                        .iter()
                        .position(|entry| entry.id == target)
                        .ok_or_else(|| HookRegistrationError::TargetNotRegistered(target.into()))?;
                    if options.before.is_some() {
                        index
                    } else {
                        index + 1
                    }
                }
            };
            entries.insert(insert_at, entry);
        }
        let entries = Arc::clone(&self.entries);
        Ok(to_disposable(move || {
            let mut entries = entries.lock().unwrap();
            if let Some(index) = entries.iter().position(|entry| entry.token == token) {
                entries.remove(index);
            }
        }))
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut entries = self.entries.lock().unwrap();
        let Some(index) = entries.iter().position(|entry| entry.id == id) else {
            return false;
        };
        entries.remove(index);
        true
    }

    // Original: OrderedHookSlot.asDisposable(). It removes whichever entry
    // currently owns the ID at disposal time.
    pub fn as_disposable(&self, id: impl Into<String>) -> DisposableHandle {
        let id = id.into();
        let entries = Arc::clone(&self.entries);
        to_disposable(move || {
            let mut entries = entries.lock().unwrap();
            if let Some(index) = entries.iter().position(|entry| entry.id == id) {
                entries.remove(index);
            }
        })
    }

    // Original: OrderedHookSlot.run(). Entries are snapshotted before the
    // first handler so registration changes affect only later runs.
    pub async fn run(&self, context: &mut C, terminal: Option<HookTerminal<C>>) -> HookResult {
        let entries = Arc::new(self.entries.lock().unwrap().clone());
        let terminal = terminal.unwrap_or_else(|| Arc::new(|_| Box::pin(async { Ok(()) })));
        dispatch(entries, 0, context, terminal).await
    }
}

fn dispatch<'a, C>(
    entries: Arc<Vec<HookEntry<C>>>,
    index: usize,
    context: &'a mut C,
    terminal: HookTerminal<C>,
) -> BoxFuture<'a, HookResult>
where
    C: Send + 'static,
{
    Box::pin(async move {
        let Some(entry) = entries.get(index) else {
            return terminal(context).await;
        };
        let handler = Arc::clone(&entry.handler);
        let next_entries = Arc::clone(&entries);
        let next_terminal = Arc::clone(&terminal);
        let next: HookNext<C> = Arc::new(move |context| {
            dispatch(
                Arc::clone(&next_entries),
                index + 1,
                context,
                Arc::clone(&next_terminal),
            )
        });
        handler(context, next).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(label: &'static str) -> HookHandler<Vec<&'static str>> {
        Arc::new(move |context, next| {
            Box::pin(async move {
                context.push(label);
                next(context).await
            })
        })
    }

    #[tokio::test]
    async fn registration_order_before_after_delete_and_terminal_match_source() {
        let hooks = OrderedHookSlot::new();
        hooks
            .register("middle", push("middle"), Default::default())
            .unwrap();
        hooks
            .register(
                "first",
                push("first"),
                HookRegisterOptions {
                    before: Some("middle"),
                    after: None,
                },
            )
            .unwrap();
        hooks
            .register(
                "last",
                push("last"),
                HookRegisterOptions {
                    before: None,
                    after: Some("middle"),
                },
            )
            .unwrap();
        let mut seen = Vec::new();
        hooks
            .run(
                &mut seen,
                Some(Arc::new(|context| {
                    Box::pin(async move {
                        context.push("terminal");
                        Ok(())
                    })
                })),
            )
            .await
            .unwrap();
        assert_eq!(seen, ["first", "middle", "last", "terminal"]);
        assert!(hooks.delete("middle"));
        assert!(!hooks.delete("missing"));
    }

    #[tokio::test]
    async fn replacement_disposable_does_not_remove_new_entry_and_run_uses_snapshot() {
        let hooks = Arc::new(OrderedHookSlot::new());
        let old = hooks
            .register("same", push("old"), Default::default())
            .unwrap();
        hooks
            .register("same", push("new"), Default::default())
            .unwrap();
        old.dispose().unwrap();

        let hooks_for_handler = Arc::clone(&hooks);
        hooks
            .register(
                "registrar",
                Arc::new(move |context, next| {
                    let hooks = Arc::clone(&hooks_for_handler);
                    Box::pin(async move {
                        hooks
                            .register("late", push("late"), Default::default())
                            .unwrap();
                        next(context).await
                    })
                }),
                Default::default(),
            )
            .unwrap();
        let mut first = Vec::new();
        hooks.run(&mut first, None).await.unwrap();
        assert_eq!(first, ["new"]);
        let mut second = Vec::new();
        hooks.run(&mut second, None).await.unwrap();
        assert_eq!(second, ["new", "late"]);
    }

    #[tokio::test]
    async fn handler_can_fork_context_before_continuing() {
        let hooks = OrderedHookSlot::new();
        hooks
            .register(
                "fork",
                Arc::new(|context: &mut Vec<&'static str>, next| {
                    Box::pin(async move {
                        context.push("outer");
                        let mut fork = context.clone();
                        fork.push("fork");
                        next(&mut fork).await?;
                        context.extend(fork);
                        Ok(())
                    })
                }),
                Default::default(),
            )
            .unwrap();
        hooks
            .register("downstream", push("downstream"), Default::default())
            .unwrap();
        let mut context = Vec::new();
        hooks.run(&mut context, None).await.unwrap();
        assert_eq!(context, ["outer", "outer", "fork", "downstream"]);
    }

    #[test]
    fn rejects_conflicting_or_missing_position_targets() {
        let hooks = OrderedHookSlot::<()>::new();
        assert_eq!(
            hooks
                .register(
                    "bad",
                    Arc::new(|_, _| Box::pin(async { Ok(()) })),
                    HookRegisterOptions {
                        before: Some("x"),
                        after: Some("y")
                    }
                )
                .err()
                .unwrap(),
            HookRegistrationError::ConflictingPosition
        );
        assert_eq!(
            hooks
                .register(
                    "bad",
                    Arc::new(|_, _| Box::pin(async { Ok(()) })),
                    HookRegisterOptions {
                        before: Some("missing"),
                        after: None
                    }
                )
                .err()
                .unwrap(),
            HookRegistrationError::TargetNotRegistered("missing".into())
        );
    }
}
