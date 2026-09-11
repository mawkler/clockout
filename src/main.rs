mod calc;
mod cli;
mod model;
mod parse;
mod storage;
mod tui;

use clap::Parser;
use cli::Cli;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(command) => cli::run(command),
        None => tui::run(),
    }
}
