use crate::IngestError;

pub const SEPARATOR: &str = "--------------------------------";

#[derive(Debug, Clone)]
pub struct SnippetInput {
    pub snippet_id: String,
    pub title: String,
    pub source_url: String,
    pub description: String,
    pub code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TextUnit {
    pub snippet_id: String,
    pub kind: &'static str,
    pub title: String,
    pub source_url: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ParsedCorpus {
    pub library: String,
    pub version: String,
    pub snippets: Vec<SnippetInput>,
}

pub fn parse_snapshot(
    library: &str,
    version: &str,
    text: &str,
) -> Result<ParsedCorpus, IngestError> {
    let blocks: Vec<String> = text
        .lines()
        .collect::<Vec<_>>()
        .split(|line| *line == SEPARATOR)
        .map(|lines| lines.join("\n"))
        .collect();
    if blocks.len() < 2 {
        return Err(IngestError::MissingSeparator);
    }
    let mut snippets = Vec::new();
    for (index, block) in blocks.into_iter().enumerate() {
        if block.trim().is_empty() {
            continue;
        }
        snippets.push(parse_block(
            library,
            version,
            snippets.len() + 1,
            index + 1,
            &block,
        )?);
    }
    Ok(ParsedCorpus {
        library: library.into(),
        version: version.into(),
        snippets,
    })
}

fn parse_block(
    library: &str,
    version: &str,
    ordinal: usize,
    block: usize,
    input: &str,
) -> Result<SnippetInput, IngestError> {
    let lines: Vec<&str> = input.lines().collect();
    let title_index = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .ok_or(IngestError::MissingTitle { block })?;
    let title_line = lines[title_index];
    let title = title_line
        .strip_prefix("### ")
        .filter(|title| !title.trim().is_empty())
        .ok_or(IngestError::MissingTitle { block })?
        .trim()
        .to_owned();
    let mut description_lines = Vec::new();
    let mut code_fences = Vec::new();
    let mut current_code: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let mut source_seen = false;
    let mut source_url = None;
    for (index, line) in lines.iter().enumerate() {
        if in_fence {
            if line.trim_start().starts_with("```") {
                if current_code.iter().any(|line| !line.trim().is_empty()) {
                    code_fences.push(current_code.join("\n"));
                }
                current_code.clear();
                in_fence = false;
            } else {
                current_code.push(*line);
            }
        } else if index == title_index || line.trim().is_empty() {
            continue;
        } else if line.trim_start().starts_with("```") {
            in_fence = true;
        } else if let Some(value) = line.strip_prefix("Source: ") {
            if source_seen {
                return Err(IngestError::MultipleSources { block });
            }
            source_seen = true;
            source_url = (!value.trim().is_empty()).then(|| value.trim().to_owned());
        } else {
            description_lines.push(*line);
        }
    }
    if in_fence {
        return Err(IngestError::UnterminatedFence { block });
    }
    let description = description_lines.join("\n").trim().to_owned();
    let code = (!code_fences.is_empty()).then(|| code_fences.join("\n\n"));
    let source_url = source_url.ok_or(IngestError::MissingSource { block })?;
    if description.is_empty() && code.is_none() {
        return Err(IngestError::EmptyBlock { block });
    }
    Ok(SnippetInput {
        snippet_id: format!("{library}-{version}-{ordinal:06}"),
        title,
        source_url,
        description,
        code,
    })
}

impl ParsedCorpus {
    pub fn text_units(&self) -> Vec<TextUnit> {
        self.snippets
            .iter()
            .flat_map(|snippet| {
                let prose = (!snippet.description.is_empty()).then(|| TextUnit {
                    snippet_id: snippet.snippet_id.clone(),
                    kind: "prose",
                    title: snippet.title.clone(),
                    source_url: snippet.source_url.clone(),
                    text: snippet.description.clone(),
                });
                let code = snippet.code.as_ref().map(|text| TextUnit {
                    snippet_id: snippet.snippet_id.clone(),
                    kind: "code",
                    title: snippet.title.clone(),
                    source_url: snippet.source_url.clone(),
                    text: text.clone(),
                });
                prose.into_iter().chain(code)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/snapshot.txt");

    #[test]
    fn parses_title_source_prose_and_every_code_fence() {
        let parsed = parse_snapshot("numpy", "2026-08-24", SAMPLE).unwrap();
        assert_eq!(parsed.snippets.len(), 2);
        assert_eq!(parsed.snippets[1].title, "Second example");
        assert_eq!(parsed.snippets[1].source_url, "https://example.test/second");
        assert_eq!(parsed.snippets[1].description, "Second prose.");
        assert_eq!(
            parsed.snippets[1].code.as_deref(),
            Some("two = 2\n\nthree = 3")
        );
    }

    #[test]
    fn refuses_input_without_the_exact_separator() {
        let error =
            parse_snapshot("numpy", "2026-08-24", "### Only\n\nSource: https://x\n").unwrap_err();
        assert!(matches!(error, IngestError::MissingSeparator));
    }

    #[test]
    fn refuses_a_block_without_a_source_url() {
        let input = "### First\n\nprose\n\n--------------------------------\n\n### Second\n\nSource: https://x\n";
        assert!(matches!(
            parse_snapshot("numpy", "2026-08-24", input),
            Err(IngestError::MissingSource { block: 1 })
        ));
    }

    #[test]
    fn preserves_blank_lines_inside_code_fences() {
        let input = "### Example\n\nSource: https://example.test\n\n```python\none = 1\n\ntwo = 2\n```\n--------------------------------\n";
        let parsed = parse_snapshot("numpy", "2026-08-24", input).unwrap();
        assert_eq!(
            parsed.snippets[0].code.as_deref(),
            Some("one = 1\n\ntwo = 2")
        );
    }

    #[test]
    fn refuses_a_block_with_multiple_source_urls() {
        let input = "### Example\n\nSource: https://example.test/one\nSource: https://example.test/two\n\nprose\n--------------------------------\n";
        assert!(matches!(
            parse_snapshot("numpy", "2026-08-24", input),
            Err(IngestError::MultipleSources { block: 1 })
        ));
    }

    #[test]
    fn source_inside_a_code_fence_does_not_satisfy_metadata() {
        let input = "### Example\n\n```text\nSource: https://inside-code.test\n```\n--------------------------------\n";

        assert!(matches!(
            parse_snapshot("numpy", "2026-08-24", input),
            Err(IngestError::MissingSource { block: 1 })
        ));
    }

    #[test]
    fn source_inside_code_is_preserved_when_metadata_source_exists() {
        let input = "### Example\n\nSource: https://metadata.test\n\n```text\nSource: https://inside-code.test\n```\n--------------------------------\n";

        let parsed = parse_snapshot("numpy", "2026-08-24", input).unwrap();
        assert_eq!(parsed.snippets[0].source_url, "https://metadata.test");
        assert_eq!(
            parsed.snippets[0].code.as_deref(),
            Some("Source: https://inside-code.test")
        );
    }

    #[test]
    fn empty_code_fences_do_not_create_a_code_unit() {
        let input = "### Example\n\nSource: https://example.test\n\nProse.\n\n```text\n```\n--------------------------------\n";

        let parsed = parse_snapshot("numpy", "2026-08-24", input).unwrap();
        assert_eq!(parsed.snippets[0].description, "Prose.");
        assert_eq!(parsed.snippets[0].code, None);
        assert_eq!(parsed.text_units().len(), 1);
    }

    #[test]
    fn empty_code_fences_do_not_make_an_empty_block_valid() {
        let input = "### Example\n\nSource: https://example.test\n\n```text\n```\n--------------------------------\n";

        assert!(matches!(
            parse_snapshot("numpy", "2026-08-24", input),
            Err(IngestError::EmptyBlock { block: 1 })
        ));
    }
}
