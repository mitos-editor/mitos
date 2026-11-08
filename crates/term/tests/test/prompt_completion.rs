//! Delivery of prompt completions computed on a background thread.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

use term::{
    compositor::{Component, Context},
    job::Jobs,
    ui::{
        completers::CompletionResult,
        prompt::{Completion, CompletionDirection},
        Prompt,
    },
};
use tui::text::Span;

use super::helpers::{test_config, AppBuilder};

struct Fixture {
    app: term::application::TestApplication,
    config: term::config::Config,
    jobs: Jobs,
    updates: tokio::sync::mpsc::UnboundedSender<term::config::ConfigEvent>,
}

impl Fixture {
    fn new() -> anyhow::Result<Self> {
        let config = test_config();
        Ok(Self {
            app: AppBuilder::new().with_config(config.clone()).build()?,
            config,
            jobs: Jobs::new(),
            updates: tokio::sync::mpsc::unbounded_channel().0,
        })
    }

    fn context(&mut self) -> Context<'_> {
        Context {
            config: term::config::Context {
                current: &self.config,
                updates: &self.updates,
            },
            editor: &mut self.app.editor,
            interaction: &mut self.app.interaction,
            jobs: &mut self.jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: false,
        }
    }
}

/// Keeps a deferred computation running until the test releases it, which is how a test
/// reaches the state where the prompt has already given up waiting for it.
#[derive(Clone, Default)]
struct Gate(Arc<AtomicBool>);

impl Gate {
    fn release(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn wait(&self) {
        // Bounded so that a test which never releases the gate can still tear its runtime
        // down instead of waiting on this blocking task forever.
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.0.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Completer computing `completion for <input>`, blocking on `gate` while doing so and
/// reporting each time it starts on the `started` channel.
fn deferred_completions(
    gate: Gate,
    started: mpsc::Sender<()>,
    running: Arc<AtomicUsize>,
) -> impl FnMut(&view::Editor, &str) -> CompletionResult {
    move |_editor, input| {
        let (gate, started, running) = (gate.clone(), started.clone(), running.clone());
        let input = input.to_owned();
        CompletionResult::Deferred(Box::new(move |_: &event::TaskHandle| -> Vec<Completion> {
            running.fetch_add(1, Ordering::SeqCst);
            let _ = started.send(());
            gate.wait();
            running.fetch_sub(1, Ordering::SeqCst);
            vec![(0.., Span::raw(format!("completion for {input}")))]
        }))
    }
}

/// The line as it would be after selecting the first completion: still the typed text while
/// nothing has been delivered, since selecting from an empty completion list does nothing.
fn selected(prompt: &mut Prompt) -> String {
    prompt.change_completion_selection(CompletionDirection::Forward);
    prompt.line().clone()
}

/// Drive the prompt until `expected` is selectable. A background task reports through a
/// redraw, so no input tells the test that it happened.
fn wait_for_completion(prompt: &mut Prompt, fixture: &mut Fixture, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut cx = fixture.context();
        prompt.update(view::graphics::Rect::default(), &mut cx);
        drop(cx);
        let selection = selected(prompt);
        if selection == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a deferred completion, last selected {selection:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_applies_deferred_completions_without_further_input() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let gate = Gate::default();
    let (started, started_rx) = mpsc::channel();
    let mut prompt = Prompt::new(
        ":".into(),
        None,
        deferred_completions(gate.clone(), started, Arc::new(AtomicUsize::new(0))),
        |_, _, _| {},
    );

    prompt.set_line("first".into(), &fixture.app.editor);
    started_rx.recv_timeout(Duration::from_secs(10))?;
    // Nothing is selectable yet: the prompt handed this task over to the background.
    assert_eq!(selected(&mut prompt), "first");

    // The task reports on its own; the prompt has to collect it without new input.
    gate.release();
    wait_for_completion(&mut prompt, &mut fixture, "completion for first");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_computes_one_deferred_completion_at_a_time() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let gate = Gate::default();
    let (started, started_rx) = mpsc::channel();
    let running = Arc::new(AtomicUsize::new(0));
    let mut prompt = Prompt::new(
        ":".into(),
        None,
        deferred_completions(gate.clone(), started, running.clone()),
        |_, _, _| {},
    );

    prompt.set_line("first".into(), &fixture.app.editor);
    started_rx.recv_timeout(Duration::from_secs(10))?;
    // Typing while that task runs neither starts another one nor settles for the completion
    // of the text typed before it.
    prompt.set_line("second".into(), &fixture.app.editor);
    prompt.set_line("third".into(), &fixture.app.editor);
    assert_eq!(running.load(Ordering::SeqCst), 1);
    assert_eq!(selected(&mut prompt), "third");

    // Each input names itself, so a request dropped on the way here would be the wrong
    // completion rather than a timeout.
    gate.release();
    wait_for_completion(&mut prompt, &mut fixture, "completion for third");
    assert_eq!(running.load(Ordering::SeqCst), 0);

    Ok(())
}
