#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Core,
    Session,
    Agent,
}
