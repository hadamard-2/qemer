//! Guided creation and editing of Qemer's own configuration file.

use color_eyre::Result;
use dialoguer::Input;

use crate::config::{self, ConfigDraft};

fn text_input(prompt: &str, default: Option<String>) -> Result<String> {
    Ok(match default {
        Some(default) => Input::new()
            .with_prompt(prompt)
            .default(default)
            .interact_text()?,
        None => Input::new().with_prompt(prompt).interact_text()?,
    })
}

fn number_input(prompt: &str, default: Option<usize>) -> Result<usize> {
    Ok(match default {
        Some(default) => Input::new()
            .with_prompt(prompt)
            .default(default)
            .interact_text()?,
        None => Input::new().with_prompt(prompt).interact_text()?,
    })
}

/// Prompt for every model-facing setting and atomically save a valid config.
pub fn run() -> Result<()> {
    let path = config::resolve_path(std::env::var("QEMER_CONFIG").ok())?;
    let exists = path.exists();
    let mut draft: ConfigDraft = if exists {
        config::load_path(&path)?.into()
    } else {
        config::default_draft()
    };

    draft.embedding.base_url = text_input(
        "embedding.base_url",
        exists.then(|| draft.embedding.base_url.clone()),
    )?;
    draft.embedding.model = text_input(
        "embedding.model",
        exists.then(|| draft.embedding.model.clone()),
    )?;
    draft.embedding.dim = number_input("embedding.dim", exists.then_some(draft.embedding.dim))?;
    draft.completion.base_url = text_input(
        "completion.base_url",
        exists.then(|| draft.completion.base_url.clone()),
    )?;
    draft.completion.model = text_input(
        "completion.model",
        exists.then(|| draft.completion.model.clone()),
    )?;
    draft.completion.context_tokens = number_input(
        "completion.context_tokens",
        exists.then_some(draft.completion.context_tokens),
    )?;
    draft.completion.max_completion_tokens = number_input(
        "completion.max_completion_tokens",
        exists.then_some(draft.completion.max_completion_tokens),
    )?;

    let config = config::validate_draft(draft)?;
    config::write(&path, &config)?;
    println!("saved config to {}", path.display());
    Ok(())
}
