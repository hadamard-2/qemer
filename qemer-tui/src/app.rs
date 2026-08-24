//! Application state, and what a key does to it.
//!
//! `handle_key` mutates state and returns an `Action` describing what the
//! caller should do about the outside world. Returning an intent rather than
//! performing I/O is what lets every key rule be tested without a runtime or
//! a terminal.

use crate::query::{QueryError, QueryEvent};
use qemer_core::{Corpus, Snippet};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Picker,
    Query,
}

/// Where a query has got to. `Searching` and `Streaming` both block input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Searching,
    Streaming,
    Done {
        prompt_tokens: usize,
        completion_tokens: usize,
    },
}

/// What the event loop should do about the outside world.
#[derive(Debug)]
pub enum Action {
    None,
    Quit,
    StartQuery(String),
    Abort,
}

pub struct App {
    pub screen: Screen,
    pub corpora: Vec<Corpus>,
    /// Kept as a plain index rather than a `ListState` because ratatui's
    /// `select_next` does not clamp at the end of the list. Owning the
    /// arithmetic is what makes the bounds testable.
    pub selected: usize,
    pub input: String,
    pub answer: String,
    pub sources: Vec<Snippet>,
    pub status: Status,
    pub error: Option<String>,
}

impl App {
    pub fn new(corpora: Vec<Corpus>) -> Self {
        Self {
            screen: Screen::Picker,
            corpora,
            selected: 0,
            input: String::new(),
            answer: String::new(),
            sources: Vec::new(),
            status: Status::Idle,
            error: None,
        }
    }

    pub fn selected_corpus(&self) -> Option<&Corpus> {
        self.corpora.get(self.selected)
    }

    /// Whether a query is in flight. Input is blocked exactly when this is
    /// true, which is why there is never a second stream.
    pub fn is_busy(&self) -> bool {
        matches!(self.status, Status::Searching | Status::Streaming)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        match self.screen {
            Screen::Picker => self.handle_picker_key(key),
            Screen::Query => self.handle_query_key(key),
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => {
                let last = self.corpora.len().saturating_sub(1);
                self.selected = (self.selected + 1).min(last);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Enter if self.selected_corpus().is_some() => {
                self.screen = Screen::Query;
                self.reset_answer();
                self.input.clear();
            }
            _ => {}
        }
        Action::None
    }

    fn handle_query_key(&mut self, key: KeyEvent) -> Action {
        // Esc is overloaded, resolved by state: it abandons a running query,
        // or leaves the screen when there is nothing to abandon.
        if key.code == KeyCode::Esc {
            if self.is_busy() {
                self.status = Status::Idle;
                return Action::Abort;
            }
            self.screen = Screen::Picker;
            return Action::None;
        }
        if self.is_busy() {
            return Action::None;
        }
        match key.code {
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                let query = self.input.trim().to_string();
                if query.is_empty() {
                    return Action::None;
                }
                self.reset_answer();
                self.status = Status::Searching;
                return Action::StartQuery(query);
            }
            _ => {}
        }
        Action::None
    }

    /// One live answer at a time: asking again replaces what was there.
    fn reset_answer(&mut self) {
        self.answer.clear();
        self.sources.clear();
        self.error = None;
        self.status = Status::Idle;
    }

    /// Fold one stream item into the state.
    pub fn apply(&mut self, event: Result<QueryEvent, QueryError>) {
        match event {
            Ok(QueryEvent::Searching) => self.status = Status::Searching,
            Ok(QueryEvent::Snippets(snippets)) => {
                self.sources = snippets;
                self.status = Status::Streaming;
            }
            Ok(QueryEvent::Token(text)) => {
                self.status = Status::Streaming;
                self.answer.push_str(&text);
            }
            Ok(QueryEvent::Done { prompt_tokens, completion_tokens }) => {
                self.status = Status::Done { prompt_tokens, completion_tokens };
            }
            Err(error) => {
                // Idle rather than Done: a failure must unlock the input line
                // so the user can retype, which is the whole point of being
                // able to escape a bad answer.
                self.error = Some(error.to_string());
                self.status = Status::Idle;
            }
        }
    }
}

use crate::config::Config;
use crate::{query, view};
use color_eyre::Result;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, EventStream};

/// One live query. Boxing erases the async-stream's concrete type, and `Send`
/// because the tokio runtime may move futures between worker threads.
type QueryStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<QueryEvent, QueryError>> + Send>>;

/// Run until the user quits.
///
/// The whole query lives in one `Option<Stream>`. Aborting is setting it to
/// `None`: the drop closes the connection, which stops the server generating
/// and unlocks the input line. That is the entire cancellation story, and it
/// is why no abort API was needed from either library crate.
pub async fn run(
    terminal: &mut DefaultTerminal,
    config: &Config,
    corpora: Vec<Corpus>,
) -> Result<()> {
    let mut app = App::new(corpora);
    let mut keys = EventStream::new();
    let mut running: Option<QueryStream> = None;

    loop {
        terminal.draw(|frame| view::draw(frame, &app))?;

        let action = if let Some(stream) = running.as_mut() {
            tokio::select! {
                event = keys.next() => handle_terminal_event(&mut app, event),
                item = stream.next() => {
                    match item {
                        Some(item) => {
                            app.apply(item);
                            Action::None
                        }
                        // The stream ended on its own; there is nothing left
                        // to hold.
                        None => {
                            running = None;
                            Action::None
                        }
                    }
                }
            }
        } else {
            let event = keys.next().await;
            handle_terminal_event(&mut app, event)
        };

        match action {
            Action::Quit => return Ok(()),
            Action::Abort => running = None,
            Action::StartQuery(text) => {
                let Some(corpus) = app.selected_corpus() else {
                    continue;
                };
                // Cloned so the stream owns everything and borrows no part of
                // `app`, which the loop keeps mutating.
                let corpus = Corpus {
                    reference: corpus.reference.clone(),
                    path: corpus.path.clone(),
                };
                running = Some(Box::pin(query::run(
                    corpus,
                    config.embed_client(),
                    config.generator(),
                    text,
                    config.retrieval.k,
                )));
            }
            Action::None => {}
        }
    }
}

fn handle_terminal_event(
    app: &mut App,
    event: Option<std::io::Result<Event>>,
) -> Action {
    match event {
        Some(Ok(Event::Key(key))) if key.is_press() => app.handle_key(key),
        // A closed terminal event stream means the terminal went away.
        None => Action::Quit,
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qemer_core::CorpusRef;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn a_corpus(library: &str) -> Corpus {
        Corpus {
            reference: CorpusRef {
                library: library.into(),
                version: "0.1.0".into(),
                url: "https://example/x.tar.zst".into(),
                sha256: "abc".into(),
                bytes: 1,
                embedding_model: "nomic-embed-text-v1.5".into(),
                embedding_dim: 768,
                snippet_count: 3,
            },
            path: std::path::PathBuf::from("/tmp/x"),
        }
    }

    fn an_app() -> App {
        App::new(vec![a_corpus("alpha"), a_corpus("beta"), a_corpus("gamma")])
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn on_query_screen() -> App {
        let mut app = an_app();
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.screen, Screen::Query));
        app
    }

    #[test]
    fn a_fresh_app_starts_on_the_picker_with_the_first_row_selected() {
        let app = an_app();
        assert!(matches!(app.screen, Screen::Picker));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn moving_down_and_up_walks_the_list() {
        let mut app = an_app();
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected, 1);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected, 2);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected, 1);
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected, 0);
    }

    /// ratatui's ListState does not clamp, so the bounds are ours to enforce
    /// and therefore ours to test.
    #[test]
    fn the_selection_does_not_run_off_either_end() {
        let mut app = an_app();
        for _ in 0..10 {
            app.handle_key(key(KeyCode::Up));
        }
        assert_eq!(app.selected, 0);
        for _ in 0..10 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(app.selected, 2, "three corpora means the last index is 2");
    }

    #[test]
    fn an_empty_picker_has_no_selected_corpus_and_enter_does_nothing() {
        let mut app = App::new(vec![]);
        assert!(app.selected_corpus().is_none());
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.screen, Screen::Picker));
    }

    #[test]
    fn quitting_from_the_picker_is_requested_not_performed() {
        let mut app = an_app();
        assert!(matches!(app.handle_key(key(KeyCode::Char('q'))), Action::Quit));
    }

    #[test]
    fn typing_accumulates_into_the_input_line() {
        let mut app = on_query_screen();
        for c in "how?".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(app.input, "how?");
        app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.input, "how");
    }

    #[test]
    fn enter_with_text_asks_the_caller_to_start_a_query() {
        let mut app = on_query_screen();
        for c in "search".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        match app.handle_key(key(KeyCode::Enter)) {
            Action::StartQuery(q) => assert_eq!(q, "search"),
            other => panic!("expected StartQuery, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_an_empty_line_does_nothing() {
        let mut app = on_query_screen();
        assert!(matches!(app.handle_key(key(KeyCode::Enter)), Action::None));
    }

    /// Generation blocks input, which is why there is never a second stream
    /// and nothing to interleave.
    #[test]
    fn enter_is_ignored_while_a_query_is_in_flight() {
        let mut app = on_query_screen();
        app.input = "another".into();
        app.status = Status::Streaming;
        assert!(matches!(app.handle_key(key(KeyCode::Enter)), Action::None));
    }

    #[test]
    fn typing_is_ignored_while_a_query_is_in_flight() {
        let mut app = on_query_screen();
        app.status = Status::Searching;
        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.input, "", "input is blocked, not queued");
    }

    #[test]
    fn escape_aborts_while_a_query_is_in_flight() {
        let mut app = on_query_screen();
        app.status = Status::Streaming;
        assert!(matches!(app.handle_key(key(KeyCode::Esc)), Action::Abort));
        assert!(matches!(app.screen, Screen::Query), "abort does not navigate");
    }

    #[test]
    fn escape_returns_to_the_picker_while_idle() {
        let mut app = on_query_screen();
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.screen, Screen::Picker));
    }

    #[test]
    fn ctrl_c_quits_from_anywhere() {
        let mut app = on_query_screen();
        app.status = Status::Streaming;
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(app.handle_key(event), Action::Quit));
    }

    #[test]
    fn starting_a_query_clears_the_previous_answer_and_sources() {
        let mut app = on_query_screen();
        app.answer = "old answer".into();
        app.sources = vec![];
        app.error = Some("old error".into());
        app.input = "new".into();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.answer, "", "one live answer at a time");
        assert!(app.error.is_none());
        assert!(matches!(app.status, Status::Searching));
    }

    #[test]
    fn events_accumulate_tokens_and_finish_idle() {
        let mut app = on_query_screen();
        app.apply(Ok(QueryEvent::Searching));
        assert!(app.is_busy());
        app.apply(Ok(QueryEvent::Snippets(vec![])));
        app.apply(Ok(QueryEvent::Token("Call ".into())));
        app.apply(Ok(QueryEvent::Token("search".into())));
        assert_eq!(app.answer, "Call search");
        app.apply(Ok(QueryEvent::Done { prompt_tokens: 44, completion_tokens: 3 }));
        assert!(!app.is_busy(), "a finished query unlocks the input line");
    }

    #[test]
    fn a_failed_query_shows_the_message_and_unlocks_the_input_line() {
        let mut app = on_query_screen();
        app.apply(Ok(QueryEvent::Searching));
        app.apply(Err(QueryError::Retrieval("embedding server is down".into())));
        assert_eq!(app.error.as_deref(), Some("embedding server is down"));
        assert!(!app.is_busy(), "a failure must not leave the interface stuck");
    }
}
