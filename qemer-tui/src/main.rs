//! Qemer TUI: pick a library, ask a question, read the sources and the answer.

mod app;
mod cli;
mod config;
mod query;
mod view;

use clap::Parser;
use color_eyre::Result;
use qemer_core::Cache;

#[derive(Debug, clap::Parser)]
#[command(
    name = "qemer",
    about = "Offline coding help grounded in documentation",
    version
)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Option<cli::Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    // Parsed before the config is read, so `--help` and `--version` work on a
    // machine that has never been configured.
    let args = Args::parse();

    match args.command {
        Some(cli::Command::Available { manifest }) => return cli::available(&manifest).await,
        Some(cli::Command::Install { target, manifest }) => {
            return cli::install(&target, &manifest).await;
        }
        Some(cli::Command::List) => return cli::list(),
        None => {}
    }

    let config = config::load()?;
    let cache = Cache::new(Cache::default_root()?);
    let corpora = cache.installed()?;

    // Restore the terminal even on a panic. Without this, a crash leaves the
    // shell in raw mode with no echo, which looks like a hung machine.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        hook(info);
    }));

    let mut terminal = ratatui::init();
    let outcome = app::run(&mut terminal, &config, corpora).await;
    ratatui::restore();
    outcome
}
