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
        Screen::Excerpt => draw_excerpt(frame, app),
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
            Line::from("    qemer install <library>@<version> --manifest <path-or-https-url>"),
            Line::from(""),
            Line::from("    qemer list    shows what is already installed"),
        ];
        let block = Block::default().borders(Borders::ALL).title(" Qemer ");
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let [_top_padding, logo_area, list_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(7),
        Constraint::Min(1),
    ])
    .areas(area);
    let logo = [
        "  ██████╗ ███████╗███╗   ███╗███████╗██████╗",
        " ██╔═══██╗██╔════╝████╗ ████║██╔════╝██╔══██╗",
        " ██║   ██║█████╗  ██╔████╔██║█████╗  ██████╔╝",
        " ██║▄▄ ██║██╔══╝  ██║╚██╔╝██║██╔══╝  ██╔══██╗",
        " ╚██████╔╝███████╗██║ ╚═╝ ██║███████╗██║  ██║",
        "  ╚══▀▀═╝ ╚══════╝╚═╝     ╚═╝╚══════╝╚═╝  ╚═╝",
    ];
    frame.render_widget(Paragraph::new(logo.join("\n")), logo_area);

    let items: Vec<ListItem> = app
        .corpora
        .iter()
        .map(|corpus| {
            ListItem::new(Line::from(format!(
                "{} {} · {} snippets",
                corpus.reference.library, corpus.reference.version, corpus.reference.snippet_count
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
    frame.render_stateful_widget(list, list_area, &mut state);
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
    let answer_block = Block::default()
        .borders(Borders::ALL)
        .title(status_title(app));
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
                let marker = if i == app.selected_source {
                    "▸ "
                } else {
                    "  "
                };
                Line::from(format!("{marker}{}. {}", i + 1, snippet.title))
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Retrieved excerpts ");
        frame.render_widget(Paragraph::new(lines).block(block), sources);
    }

    let prompt = if app.is_busy() {
        " Esc to abort ".to_string()
    } else if !app.sources.is_empty() {
        " Ask — ↑/↓ select excerpt, Enter open, Esc for the corpus list ".to_string()
    } else {
        " Ask — Enter to send, Esc for the corpus list ".to_string()
    };
    let input_block = Block::default().borders(Borders::ALL).title(prompt);
    let input_text = if app.is_busy() {
        app.input.clone()
    } else {
        format!("{}▌", app.input)
    };
    frame.render_widget(Paragraph::new(input_text).block(input_block), input);
}

fn draw_excerpt(frame: &mut Frame, app: &App) {
    let Some(snippet) = app.sources.get(app.selected_source) else {
        return;
    };

    let mut text = snippet.title.clone();
    if !snippet.description.is_empty() {
        text.push_str("\n\n");
        text.push_str(&snippet.description);
    }
    if let Some(code) = &snippet.code {
        text.push_str("\n\n");
        text.push_str(code);
    }

    let title = format!(
        " Excerpt {} of {} — ↑/↓ browse, Esc to answer ",
        app.selected_source + 1,
        app.sources.len()
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        frame.area(),
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
        Status::Done {
            prompt_tokens,
            completion_tokens,
        } => {
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
    use qemer_core::{Corpus, CorpusRef, Snippet};
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

    #[test]
    fn the_picker_shows_the_qemer_pixel_banner() {
        let screen = rendered(&App::new(vec![a_corpus()]));
        assert!(screen.contains("██████╗"), "{screen}");
        assert!(screen.contains("███╗   ███╗"), "{screen}");
    }

    #[test]
    fn the_picker_left_aligns_the_qemer_pixel_banner() {
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal
            .draw(|frame| draw(frame, &App::new(vec![a_corpus()])))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(2, 0)].symbol(), " ");
        assert_eq!(buffer[(2, 1)].symbol(), "█");
    }

    /// A fresh user has no corpora and this slice offers no way in the
    /// interface to get one, so the empty state must name the exact command.
    #[test]
    fn the_empty_picker_names_the_install_command() {
        let screen = rendered(&App::new(vec![]));
        assert!(
            screen.contains("qemer install <library>@<version> --manifest <path-or-https-url>"),
            "{screen}"
        );
    }

    #[test]
    fn an_editable_query_shows_a_visible_cursor() {
        let mut app = App::new(vec![a_corpus()]);
        app.screen = Screen::Query;
        app.input = "how do I create an array".into();
        let screen = rendered(&app);
        assert!(screen.contains("how do I create an array▌"), "{screen}");
    }

    #[test]
    fn query_screen_shows_the_answer_without_source_origin_urls() {
        let mut app = App::new(vec![a_corpus()]);
        app.screen = Screen::Query;
        app.answer = "Use np.zeros for a zero-filled array.".into();
        app.sources = vec![Snippet {
            library: "lancedb".into(),
            version: "0.37.1".into(),
            snippet_id: "s1".into(),
            title: "Create an array".into(),
            description: "The installed excerpt explains array creation.".into(),
            code: Some("np.zeros((4, 4), dtype=np.float32)".into()),
            source_url: Some("https://example.invalid/original-docs".into()),
            score: 1.0,
        }];

        let screen = rendered(&app);
        assert!(
            screen.contains("Use np.zeros for a zero-filled array."),
            "{screen}"
        );
        assert!(screen.contains("Create an array"), "{screen}");
        assert!(
            !screen.contains("https://example.invalid/original-docs"),
            "source origins are not rendered in the offline interface: {screen}"
        );
    }

    #[test]
    fn excerpt_screen_shows_local_content_without_the_origin_url() {
        let mut app = App::new(vec![a_corpus()]);
        app.screen = Screen::Excerpt;
        app.sources = vec![Snippet {
            library: "lancedb".into(),
            version: "0.37.1".into(),
            snippet_id: "s1".into(),
            title: "Create an array".into(),
            description: "The installed excerpt explains array creation.".into(),
            code: Some("np.zeros((4, 4), dtype=np.float32)".into()),
            source_url: Some("https://example.invalid/original-docs".into()),
            score: 1.0,
        }];

        let screen = rendered(&app);
        assert!(screen.contains("Excerpt 1 of 1"), "{screen}");
        assert!(screen.contains("installed excerpt explains"), "{screen}");
        assert!(screen.contains("np.zeros((4, 4)"), "{screen}");
        assert!(
            !screen.contains("https://example.invalid/original-docs"),
            "{screen}"
        );
    }
}
