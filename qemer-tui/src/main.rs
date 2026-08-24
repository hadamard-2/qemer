//! Qemer TUI: pick a library, ask a question, read the sources and the answer.

mod app;
mod cli;
mod config;
mod query;

use clap::Parser;
use color_eyre::Result;

#[derive(Debug, clap::Parser)]
#[command(name = "qemer", about = "Offline coding help grounded in documentation")]
struct Args {
    #[command(subcommand)]
    command: Option<cli::Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();

    // Configuration is read before anything touches the terminal, so a
    // validation failure lands on an ordinary screen the user can read.
    let config = config::load()?;

    match args.command {
        Some(cli::Command::Install { target }) => cli::install(&config, &target).await,
        Some(cli::Command::List) => cli::list(&config),
        None => {
            println!("qemer: TUI not wired up yet");
            Ok(())
        }
    }
}
