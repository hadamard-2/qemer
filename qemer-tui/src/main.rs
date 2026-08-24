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
#[command(name = "qemer", about = "Offline coding help grounded in documentation")]
struct Args {
    #[command(subcommand)]
    command: Option<cli::Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    // Parsed before the config is read, so `--help` and `--version` work on a
    // machine that has never been configured.
    let args = Args::parse();
    let config = config::load()?;

    match args.command {
        Some(cli::Command::Install { target }) => return cli::install(&config, &target).await,
        Some(cli::Command::List) => return cli::list(&config),
        None => {}
    }

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
