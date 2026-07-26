pub mod bash;
pub mod glob;
pub mod grep;
pub mod process_task;
pub mod read;
pub mod rg_locator;
mod rg_probe;
pub mod run_rg;
pub mod write;

pub use bash::*;
pub use glob::*;
pub use grep::*;
pub use read::*;
pub use write::*;

pub fn register_node_local_tools() {
    register_bash_tool();
    register_read_tool();
    register_write_tool();
    register_glob_tool();
    register_grep_tool();
}
