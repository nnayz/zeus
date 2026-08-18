pub mod args;
pub mod catalog;
pub mod commands;
pub mod conn;
pub mod error;
pub mod lineage;
pub mod mcp;
pub mod render;
pub mod support;

use crate::args::Command;
use crate::error::CliError;

pub fn run(argv: Vec<String>) -> Result<i32, CliError> {
    Command::parse(&argv)?.run()
}
