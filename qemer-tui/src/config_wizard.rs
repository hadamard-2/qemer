//! Guided creation and editing of Qemer's own configuration file.

use color_eyre::Result;
use dialoguer::Input;

use crate::config::{self, ConfigDraft};

#[derive(Debug, PartialEq, Eq)]
struct PromptDefaults {
    embedding_base_url: Option<String>,
    embedding_model: Option<String>,
    embedding_dim: Option<usize>,
    completion_base_url: Option<String>,
    completion_model: Option<String>,
    context_tokens: Option<usize>,
    max_completion_tokens: Option<usize>,
}

fn prompt_defaults(draft: &ConfigDraft, editing: bool) -> PromptDefaults {
    PromptDefaults {
        embedding_base_url: Some(draft.embedding.base_url.clone()),
        embedding_model: Some(draft.embedding.model.clone()),
        embedding_dim: Some(draft.embedding.dim),
        completion_base_url: Some(draft.completion.base_url.clone()),
        completion_model: Some(draft.completion.model.clone()),
        context_tokens: editing.then_some(draft.completion.context_tokens),
        max_completion_tokens: editing.then_some(draft.completion.max_completion_tokens),
    }
}

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
    let defaults = prompt_defaults(&draft, exists);

    draft.embedding.base_url = text_input("embedding.base_url", defaults.embedding_base_url)?;
    draft.embedding.model = text_input("embedding.model", defaults.embedding_model)?;
    draft.embedding.dim = number_input("embedding.dim", defaults.embedding_dim)?;
    draft.completion.base_url = text_input("completion.base_url", defaults.completion_base_url)?;
    draft.completion.model = text_input("completion.model", defaults.completion_model)?;
    draft.completion.context_tokens =
        number_input("completion.context_tokens", defaults.context_tokens)?;
    draft.completion.max_completion_tokens = number_input(
        "completion.max_completion_tokens",
        defaults.max_completion_tokens,
    )?;

    let config = config::validate_draft(draft)?;
    config::write(&path, &config)?;
    println!("saved config to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_prompts_default_every_builtin_value_but_token_limits() {
        let defaults = prompt_defaults(&config::default_draft(), false);

        assert_eq!(
            defaults.embedding_base_url.as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(
            defaults.embedding_model.as_deref(),
            Some("nomic-embed-text-v1.5")
        );
        assert_eq!(defaults.embedding_dim, Some(768));
        assert_eq!(
            defaults.completion_base_url.as_deref(),
            Some("http://localhost:8081")
        );
        assert_eq!(defaults.completion_model.as_deref(), Some("qwen3.5-0.8b"));
        assert_eq!(defaults.context_tokens, None);
        assert_eq!(defaults.max_completion_tokens, None);
    }

    #[test]
    fn editing_a_config_defaults_every_prompt_to_its_current_value() {
        let mut draft = config::default_draft();
        draft.completion.context_tokens = 4096;
        draft.completion.max_completion_tokens = 512;

        let defaults = prompt_defaults(&draft, true);

        assert_eq!(defaults.context_tokens, Some(4096));
        assert_eq!(defaults.max_completion_tokens, Some(512));
    }
}
