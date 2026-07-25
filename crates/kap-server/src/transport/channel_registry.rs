#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMemberKind {
    Method,
    Property,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMethodDescriptor {
    pub name: String,
    pub kind: ChannelMemberKind,
    pub arity: u32,
    pub params: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelScope {
    App,
    Session,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelDescriptor {
    pub name: String,
    pub scope: ChannelScope,
    pub domain: String,
    pub methods: Vec<ChannelMethodDescriptor>,
}

pub fn describe_all_channels() -> Vec<ChannelDescriptor> {
    // MIGRATION-TODO:
    // Original: transport/channelRegistry.ts, describeAllChannels()
    // Missing dependency: agent-core-v2 scoped service descriptor registry and
    // Rust-side reflection metadata for registered service methods.
    todo!("describe channels after agent-core-v2 DI registry is complete")
}
