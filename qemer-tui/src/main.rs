//! Qemer TUI: pick a library, ask a question, read the sources and the answer.

use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    println!("qemer: skeleton — no UI yet");
    Ok(())
}
