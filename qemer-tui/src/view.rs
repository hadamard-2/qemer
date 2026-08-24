//! Rendering. Takes `&App`, draws, and holds nothing.
//!
//! Keeping state out of this file is what lets every decision worth
//! asserting live somewhere a test can reach without a terminal.

use crate::app::{App, Screen, Status};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen {
        Screen::Picker => draw_picker(frame, app),
        Screen::Query => draw_query(frame, app),
    }
}

fn draw_picker(frame: &mut Frame, app: &App) {
    let area = frame.area();

    if app.corpora.is_empty() {
        let text = vec![
            Line::from("No corpora installed."),
            Line::from(""),
            Line::from("Install one from the command line, then start qemer again:"),
            Line::from(""),
            Line::from("    qemer install <library>@<version>"),
            Line::from(""),
            Line::from("    qemer list    shows what is already installed"),
        ];
        let block = Block::default().borders(Borders::ALL).title(" Qemer ");
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let items: Vec<ListItem> = app
        .corpora
        .iter()
        .map(|corpus| {
            ListItem::new(Line::from(format!(
                "{} {} · {} snippets",
                corpus.reference.library,
                corpus.reference.version,
                corpus.reference.snippet_count
            )))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Installed corpora — ↑/↓ to move, Enter to open, q to quit ");
    let list = List::new(items)
        .block(block)
        .highlight_symbol("▸ ")
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_query(frame: &mut Frame, app: &App) {
    let [header, body, sources, input] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(sources_height(app)),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    let title = match app.selected_corpus() {
        Some(corpus) => format!(
            " {} {} ",
            corpus.reference.library, corpus.reference.version
        ),
        None => " qemer ".to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::REVERSED),
        ))),
        header,
    );

    // The generated answer is the primary content, with the sources beneath
    // it. If the sources-first fallback in docs/decisions.md is ever taken,
    // this is the weighting that changes.
    let answer_text = if let Some(error) = &app.error {
        error.clone()
    } else if app.answer.is_empty() {
        match app.status {
            Status::Searching => "Searching…".to_string(),
            Status::Streaming => String::new(),
            _ => "Ask a question about this library.".to_string(),
        }
    } else {
        app.answer.clone()
    };
    let answer_block = Block::default().borders(Borders::ALL).title(status_title(app));
    frame.render_widget(
        Paragraph::new(answer_text)
            .block(answer_block)
            .wrap(Wrap { trim: false }),
        body,
    );

    if !app.sources.is_empty() {
        let lines: Vec<Line> = app
            .sources
            .iter()
            .enumerate()
            .map(|(i, snippet)| {
                let url = snippet.source_url.as_deref().unwrap_or("");
                Line::from(format!("{}. {}  {}", i + 1, snippet.title, url))
            })
            .collect();
        let block = Block::default().borders(Borders::ALL).title(" Sources ");
        frame.render_widget(Paragraph::new(lines).block(block), sources);
    }

    let prompt = if app.is_busy() {
        " Esc to abort ".to_string()
    } else {
        " Ask — Enter to send, Esc for the corpus list ".to_string()
    };
    let input_block = Block::default().borders(Borders::ALL).title(prompt);
    frame.render_widget(
        Paragraph::new(app.input.as_str()).block(input_block),
        input,
    );
}

/// Two lines of chrome plus one per source, capped so a long list cannot
/// squeeze the answer off the screen.
fn sources_height(app: &App) -> u16 {
    if app.sources.is_empty() {
        return 0;
    }
    (app.sources.len() as u16 + 2).min(9)
}

fn status_title(app: &App) -> String {
    match app.status {
        Status::Idle => " Answer ".to_string(),
        Status::Searching => " Answer — searching ".to_string(),
        Status::Streaming => " Answer — generating, Esc to abort ".to_string(),
        Status::Done { prompt_tokens, completion_tokens } => {
            // Zeroes mean the server reported no usage, not that nothing
            // happened; saying so beats showing "0 tokens".
            if prompt_tokens == 0 && completion_tokens == 0 {
                " Answer — complete ".to_string()
            } else {
                format!(" Answer — {prompt_tokens} prompt, {completion_tokens} generated ")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use qemer_core::{Corpus, CorpusRef};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn a_corpus() -> Corpus {
        Corpus {
            reference: CorpusRef {
                library: "lancedb".into(),
                version: "0.37.1".into(),
                url: "https://example/x.tar.zst".into(),
                sha256: "abc".into(),
                bytes: 1,
                embedding_model: "nomic-embed-text-v1.5".into(),
                embedding_dim: 768,
                snippet_count: 4213,
            },
            path: std::path::PathBuf::from("/tmp/x"),
        }
    }

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn the_picker_shows_each_installed_corpus() {
        let screen = rendered(&App::new(vec![a_corpus()]));
        assert!(screen.contains("lancedb"), "{screen}");
        assert!(screen.contains("0.37.1"), "{screen}");
    }

    /// A fresh user has no corpora and this slice offers no way in the
    /// interface to get one, so the empty state must name the exact command.
    #[test]
    fn the_empty_picker_names_the_install_command() {
        let screen = rendered(&App::new(vec![]));
        assert!(screen.contains("qemer install"), "{screen}");
    }
}
