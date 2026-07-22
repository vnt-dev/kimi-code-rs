use std::{
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
};

pub type BoxError = Box<dyn Error + Send + Sync>;
pub type ActionFuture = Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>>;
pub type LifecycleAction = Box<dyn FnOnce() -> ActionFuture + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleTransitionErrorReason {
    InvalidState,
    TransitionConflict,
    MissingCommitState,
    MissingRollbackState,
    AlreadyCommitted,
    AlreadyRolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleTransitionError<S> {
    pub reason: LifecycleTransitionErrorReason,
    pub operation: String,
    pub state: S,
    pub expected: Option<Vec<S>>,
    pub active_operation: Option<String>,
}

impl<S: fmt::Display> fmt::Display for LifecycleTransitionError<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            LifecycleTransitionErrorReason::InvalidState => write!(
                formatter,
                "Lifecycle operation \"{}\" is not allowed from state \"{}\"",
                self.operation, self.state
            ),
            LifecycleTransitionErrorReason::TransitionConflict => write!(
                formatter,
                "Lifecycle operation \"{}\" conflicts with active operation \"{}\"",
                self.operation,
                self.active_operation.as_deref().unwrap_or_default()
            ),
            LifecycleTransitionErrorReason::MissingCommitState => write!(
                formatter,
                "Lifecycle operation \"{}\" did not select a commit state",
                self.operation
            ),
            LifecycleTransitionErrorReason::MissingRollbackState => write!(
                formatter,
                "Lifecycle operation \"{}\" did not select a rollback state",
                self.operation
            ),
            LifecycleTransitionErrorReason::AlreadyCommitted => write!(
                formatter,
                "Lifecycle operation \"{}\" already selected a commit state",
                self.operation
            ),
            LifecycleTransitionErrorReason::AlreadyRolledBack => write!(
                formatter,
                "Lifecycle operation \"{}\" already selected a rollback state",
                self.operation
            ),
        }
    }
}

impl<S> Error for LifecycleTransitionError<S> where
    S: fmt::Debug + fmt::Display + Send + Sync + 'static
{
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleSnapshot<S> {
    pub state: S,
    pub transitioning: bool,
    pub operation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LifecycleSwitchOptions<S> {
    pub operation: String,
    pub from: Vec<S>,
    pub to: S,
}

#[derive(Debug, Clone)]
pub struct LifecycleTransactionOptions<S> {
    pub operation: String,
    pub from: Vec<S>,
    pub enter: S,
    pub commit: Option<S>,
    pub rollback: Option<S>,
}

#[derive(Debug)]
struct MachineState<S> {
    state: S,
    active_operation: Option<String>,
}

pub struct LifecycleMachine<S> {
    inner: Arc<Mutex<MachineState<S>>>,
}

impl<S> LifecycleMachine<S>
where
    S: Clone + Eq + fmt::Debug + fmt::Display + Send + Sync + 'static,
{
    pub fn new(initial: S) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MachineState {
                state: initial,
                active_operation: None,
            })),
        }
    }

    pub fn state(&self) -> S {
        self.lock().state.clone()
    }

    pub fn snapshot(&self) -> LifecycleSnapshot<S> {
        let inner = self.lock();
        LifecycleSnapshot {
            state: inner.state.clone(),
            transitioning: inner.active_operation.is_some(),
            operation: inner.active_operation.clone(),
        }
    }

    pub fn is(&self, states: &[S]) -> bool {
        states.contains(&self.lock().state)
    }

    // Original: LifecycleMachine.switch().
    pub fn switch(
        &self,
        options: LifecycleSwitchOptions<S>,
    ) -> Result<(), LifecycleTransitionError<S>> {
        let mut inner = self.lock();
        assert_idle(&inner, &options.operation)?;
        assert_state(&inner, &options.operation, &options.from)?;
        inner.state = options.to;
        Ok(())
    }

    // Original: LifecycleMachine.transaction(). Actions retain the source LIFO ordering.
    pub async fn transaction<R, F, Fut, E>(
        &self,
        options: LifecycleTransactionOptions<S>,
        callback: F,
    ) -> Result<R, BoxError>
    where
        F: FnOnce(LifecycleTransaction<S>) -> Fut,
        Fut: Future<Output = Result<R, E>>,
        E: Error + Send + Sync + 'static,
    {
        {
            let mut inner = self.lock();
            assert_idle(&inner, &options.operation).map_err(boxed)?;
            assert_state(&inner, &options.operation, &options.from).map_err(boxed)?;
            inner.active_operation = Some(options.operation.clone());
            inner.state = options.enter.clone();
        }

        let transaction = LifecycleTransaction::new(
            Arc::clone(&self.inner),
            options.operation.clone(),
            options.commit,
            options.rollback,
        );
        let callback_result = callback(transaction.clone()).await;

        match callback_result {
            Err(error) => {
                let mut errors = vec![boxed(error)];
                errors.extend(run_actions(transaction.take_rollbacks()).await);
                errors.extend(run_actions(transaction.take_deferred()).await);
                match transaction.rollback_state() {
                    Some(state) => self.lock().state = state,
                    None => errors
                        .push(boxed(transaction.transition_error(
                            LifecycleTransitionErrorReason::MissingRollbackState,
                        ))),
                }
                transaction.finish();
                Err(aggregate_errors(
                    errors,
                    format!("Lifecycle transaction \"{}\" failed", options.operation),
                ))
            }
            Ok(result) => {
                let Some(commit_state) = transaction.commit_state() else {
                    transaction.finish_without_state_change();
                    return Err(boxed(transaction.transition_error(
                        LifecycleTransitionErrorReason::MissingCommitState,
                    )));
                };
                let mut errors = run_actions(transaction.take_deferred()).await;
                self.lock().state = commit_state;
                errors.extend(run_actions(transaction.take_after_commit()).await);
                transaction.finish();
                if errors.is_empty() {
                    Ok(result)
                } else {
                    Err(aggregate_errors(
                        errors,
                        format!(
                            "Lifecycle transaction \"{}\" committed with action failures",
                            options.operation
                        ),
                    ))
                }
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, MachineState<S>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct TransactionState<S> {
    commit_state: Option<S>,
    rollback_state: Option<S>,
    commit_selected: bool,
    rollback_selected: bool,
    deferred: Vec<LifecycleAction>,
    rollbacks: Vec<LifecycleAction>,
    after_commit: Vec<LifecycleAction>,
    finished: bool,
}

pub struct LifecycleTransaction<S> {
    machine: Arc<Mutex<MachineState<S>>>,
    operation: String,
    inner: Arc<Mutex<TransactionState<S>>>,
}

impl<S> Clone for LifecycleTransaction<S> {
    fn clone(&self) -> Self {
        Self {
            machine: Arc::clone(&self.machine),
            operation: self.operation.clone(),
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S> LifecycleTransaction<S>
where
    S: Clone + Eq + fmt::Debug + fmt::Display + Send + Sync + 'static,
{
    fn new(
        machine: Arc<Mutex<MachineState<S>>>,
        operation: String,
        commit_state: Option<S>,
        rollback_state: Option<S>,
    ) -> Self {
        Self {
            machine,
            operation,
            inner: Arc::new(Mutex::new(TransactionState {
                commit_state,
                rollback_state,
                commit_selected: false,
                rollback_selected: false,
                deferred: Vec::new(),
                rollbacks: Vec::new(),
                after_commit: Vec::new(),
                finished: false,
            })),
        }
    }

    pub fn defer(&self, action: LifecycleAction) {
        self.lock().deferred.push(action);
    }

    pub fn rollback(&self, action: LifecycleAction) {
        self.lock().rollbacks.push(action);
    }

    pub fn after_commit(&self, action: LifecycleAction) {
        self.lock().after_commit.push(action);
    }

    pub fn commit(&self, state: S) -> Result<(), LifecycleTransitionError<S>> {
        let mut inner = self.lock();
        if inner.commit_selected {
            return Err(self.transition_error(LifecycleTransitionErrorReason::AlreadyCommitted));
        }
        inner.commit_selected = true;
        inner.commit_state = Some(state);
        Ok(())
    }

    pub fn rollback_to(&self, state: S) -> Result<(), LifecycleTransitionError<S>> {
        let mut inner = self.lock();
        if inner.rollback_selected {
            return Err(self.transition_error(LifecycleTransitionErrorReason::AlreadyRolledBack));
        }
        inner.rollback_selected = true;
        inner.rollback_state = Some(state);
        Ok(())
    }

    fn transition_error(
        &self,
        reason: LifecycleTransitionErrorReason,
    ) -> LifecycleTransitionError<S> {
        LifecycleTransitionError {
            reason,
            operation: self.operation.clone(),
            state: self
                .machine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state
                .clone(),
            expected: None,
            active_operation: None,
        }
    }

    fn commit_state(&self) -> Option<S> {
        self.lock().commit_state.clone()
    }

    fn rollback_state(&self) -> Option<S> {
        self.lock().rollback_state.clone()
    }

    fn take_deferred(&self) -> Vec<LifecycleAction> {
        std::mem::take(&mut self.lock().deferred)
    }

    fn take_rollbacks(&self) -> Vec<LifecycleAction> {
        std::mem::take(&mut self.lock().rollbacks)
    }

    fn take_after_commit(&self) -> Vec<LifecycleAction> {
        std::mem::take(&mut self.lock().after_commit)
    }

    fn finish(&self) {
        self.lock().finished = true;
        self.machine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_operation = None;
    }

    fn finish_without_state_change(&self) {
        self.finish();
    }

    fn lock(&self) -> MutexGuard<'_, TransactionState<S>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<S> Drop for LifecycleTransaction<S> {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) != 1 {
            return;
        }
        let mut transaction = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if transaction.finished {
            return;
        }
        let mut machine = self
            .machine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(rollback) = transaction.rollback_state.take() {
            machine.state = rollback;
        }
        machine.active_operation = None;
        transaction.finished = true;
    }
}

#[derive(Debug)]
pub struct AggregateLifecycleError {
    message: String,
    errors: Vec<BoxError>,
}

impl AggregateLifecycleError {
    pub fn errors(&self) -> &[BoxError] {
        &self.errors
    }
}

impl fmt::Display for AggregateLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for AggregateLifecycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.errors.first().map(|error| error.as_ref() as _)
    }
}

fn assert_idle<S>(
    inner: &MachineState<S>,
    operation: &str,
) -> Result<(), LifecycleTransitionError<S>>
where
    S: Clone,
{
    if let Some(active_operation) = &inner.active_operation {
        return Err(LifecycleTransitionError {
            reason: LifecycleTransitionErrorReason::TransitionConflict,
            operation: operation.to_owned(),
            state: inner.state.clone(),
            expected: None,
            active_operation: Some(active_operation.clone()),
        });
    }
    Ok(())
}

fn assert_state<S>(
    inner: &MachineState<S>,
    operation: &str,
    expected: &[S],
) -> Result<(), LifecycleTransitionError<S>>
where
    S: Clone + Eq,
{
    if expected.contains(&inner.state) {
        return Ok(());
    }
    Err(LifecycleTransitionError {
        reason: LifecycleTransitionErrorReason::InvalidState,
        operation: operation.to_owned(),
        state: inner.state.clone(),
        expected: Some(expected.to_vec()),
        active_operation: None,
    })
}

async fn run_actions(mut actions: Vec<LifecycleAction>) -> Vec<BoxError> {
    let mut errors = Vec::new();
    while let Some(action) = actions.pop() {
        if let Err(error) = action().await {
            errors.push(error);
        }
    }
    errors
}

fn boxed(error: impl Error + Send + Sync + 'static) -> BoxError {
    Box::new(error)
}

fn aggregate_errors(mut errors: Vec<BoxError>, message: String) -> BoxError {
    if errors.len() == 1 {
        return errors.pop().expect("single error exists");
    }
    Box::new(AggregateLifecycleError { message, errors })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{convert::Infallible, sync::Mutex as StdMutex};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Idle,
        Running,
        Completed,
        Failed,
    }

    impl fmt::Display for State {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "{:?}", self).map(|_| ())
        }
    }

    fn action(f: impl FnOnce() + Send + 'static) -> LifecycleAction {
        Box::new(move || {
            f();
            Box::pin(async { Ok(()) })
        })
    }

    fn options() -> LifecycleTransactionOptions<State> {
        LifecycleTransactionOptions {
            operation: "run".into(),
            from: vec![State::Idle],
            enter: State::Running,
            commit: Some(State::Completed),
            rollback: Some(State::Failed),
        }
    }

    #[test]
    fn switches_only_from_an_allowed_idle_state() {
        let machine = LifecycleMachine::new(State::Idle);
        machine
            .switch(LifecycleSwitchOptions {
                operation: "start".into(),
                from: vec![State::Idle],
                to: State::Running,
            })
            .unwrap();
        assert_eq!(machine.state(), State::Running);
        assert_eq!(
            machine.snapshot(),
            LifecycleSnapshot {
                state: State::Running,
                transitioning: false,
                operation: None
            }
        );
    }

    #[tokio::test]
    async fn transaction_exposes_enter_state_and_commits() {
        let machine = LifecycleMachine::new(State::Idle);
        let observed = Arc::new(StdMutex::new(Vec::new()));
        let seen = Arc::clone(&observed);
        let result = machine
            .transaction(options(), |_| async move {
                seen.lock().unwrap().push(State::Running);
                Ok::<_, Infallible>(42)
            })
            .await
            .unwrap();
        assert_eq!(result, 42);
        assert_eq!(machine.state(), State::Completed);
        assert_eq!(*observed.lock().unwrap(), vec![State::Running]);
    }

    #[tokio::test]
    async fn actions_are_lifo_and_failure_rolls_back() {
        let machine = LifecycleMachine::new(State::Idle);
        let order = Arc::new(StdMutex::new(Vec::new()));
        let result = machine
            .transaction(options(), |transaction| {
                for label in ["rollback-1", "rollback-2"] {
                    let order = Arc::clone(&order);
                    transaction.rollback(action(move || order.lock().unwrap().push(label)));
                }
                for label in ["defer-1", "defer-2"] {
                    let order = Arc::clone(&order);
                    transaction.defer(action(move || order.lock().unwrap().push(label)));
                }
                async { Err::<(), _>(std::io::Error::other("boom")) }
            })
            .await;
        assert_eq!(result.unwrap_err().to_string(), "boom");
        assert_eq!(machine.state(), State::Failed);
        assert_eq!(
            *order.lock().unwrap(),
            vec!["rollback-2", "rollback-1", "defer-2", "defer-1"]
        );
    }

    #[tokio::test]
    async fn rejects_a_concurrent_transition() {
        let machine = Arc::new(LifecycleMachine::new(State::Idle));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let running_machine = Arc::clone(&machine);
        let running = tokio::spawn(async move {
            running_machine
                .transaction(options(), |_| async move {
                    release_rx.await.unwrap();
                    Ok::<_, Infallible>(())
                })
                .await
        });
        tokio::task::yield_now().await;
        let error = machine
            .switch(LifecycleSwitchOptions {
                operation: "nested".into(),
                from: vec![State::Running],
                to: State::Failed,
            })
            .unwrap_err();
        assert_eq!(
            error.reason,
            LifecycleTransitionErrorReason::TransitionConflict
        );
        release_tx.send(()).unwrap();
        running.await.unwrap().unwrap();
    }
}
