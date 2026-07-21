#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommandAvailability {
    Always,
    IdleOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommandBusyReason {
    Streaming,
    Compacting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommandInvalidReason {
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiSlashCommand {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub priority: Option<i32>,
    pub availability: Option<SlashCommandAvailability>,
    pub experimental_flag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSlashInput {
    pub name: String,
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: String,
}
