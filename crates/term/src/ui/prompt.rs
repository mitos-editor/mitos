use crate::compositor::DrawContext;
use crate::compositor::{
    Component, Compositor, Context, Cursor, DeferredEvent, Event, EventResult,
};
use crate::keymap::component::PromptAction;
use crate::ui::{
    self,
    completers::{CompletionResult, DeferredCompletion},
};
use arc_swap::ArcSwap;
use editor_core::search::SearchSyntax;
use event::TaskController;
use language::syntax;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::{borrow::Cow, ops::RangeFrom};
use std::{cell::Cell, rc::Rc};
use stdx::rope::RopeExt as _;
use tui::buffer::Buffer as Surface;
use tui::layout::Constraint;
use tui::text::Span;
use tui::widgets::{Row, StatefulWidget, Table, TableState, Widget};
use ui_core::input::KeyEvent;
use ui_core::keyboard::{KeyCode, KeyModifiers};
use ui_core::keymap::{KeymapState, Lookup};
use view::document::Mode;
use view::register::SearchHistoryEntry;

use editor_core::{
    unicode::segmentation::{GraphemeCursor, UnicodeSegmentation},
    unicode::width::UnicodeWidthStr,
};
use view::{graphics::Rect, Editor};

type PromptCharHandler = Box<dyn Fn(&mut Prompt, char, &Context)>;

pub type Completion = (RangeFrom<usize>, Span<'static>);
type CompletionFn = Box<dyn FnMut(&Editor, &str) -> CompletionResult>;
type CallbackFn =
    Box<dyn FnMut(&mut Context, &str, PromptEvent) -> Option<crate::compositor::Callback>>;
pub type DocFn = Box<dyn Fn(&str) -> Option<Cow<str>>>;

struct CompletionTask {
    results: Receiver<Vec<Completion>>,
    obsolete: bool,
}

/// Identity of one prompt instance, independent of its text and component type.
#[derive(Clone, Default)]
pub(crate) struct PromptSession(Arc<()>);

impl PromptSession {
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub struct Prompt {
    session: PromptSession,
    keys: KeymapState,
    prompt: Cow<'static, str>,
    line: String,
    cursor: usize,
    // Fields used for Component callbacks and rendering:
    line_area: Rect,
    anchor: usize,
    truncate_start: bool,
    truncate_end: bool,
    // ---
    completion: Vec<Completion>,
    completion_line: String,
    selection: Option<usize>,
    history_register: Option<char>,
    history_pos: Option<usize>,
    history_draft: Option<SearchHistoryEntry>,
    history_edited: bool,
    cached_suggestion: std::cell::OnceCell<Option<SearchHistoryEntry>>,
    search_syntax: Option<Rc<Cell<SearchSyntax>>>,
    completion_fn: CompletionFn,
    /// Cancels background work computing deferred completions when they are no longer needed.
    task_controller: TaskController,
    /// Retained until the worker reports, even after cancellation: a blocked syscall
    /// cannot be interrupted, and must not spawn another worker on every keystroke.
    deferred_completion: Option<CompletionTask>,
    /// Only the latest deferred request waits for the occupied worker slot.
    pending_completion: Option<DeferredCompletion>,
    /// A completion selection and the input following it resume together once ready.
    pending_actions: Option<(Vec<PromptAction>, Vec<char>)>,
    pending_input: Vec<DeferredEvent>,
    action_scheduled: bool,
    callback_fn: CallbackFn,
    pub doc_fn: DocFn,
    next_char_handler: Option<PromptCharHandler>,
    language: Option<(&'static str, Arc<ArcSwap<syntax::Loader>>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptEvent {
    /// The prompt input has been updated.
    Update,
    /// Validate and finalize the change.
    Validate,
    /// Abort the change, reverting to the initial state.
    Abort,
}

pub enum CompletionDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy)]
pub enum Movement {
    BackwardChar(usize),
    BackwardWord(usize),
    ForwardChar(usize),
    ForwardWord(usize),
    StartOfLine,
    EndOfLine,
    None,
}

fn is_word_sep(c: char) -> bool {
    c == std::path::MAIN_SEPARATOR || c.is_whitespace()
}

fn input_char(key: KeyEvent) -> Option<char> {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        None
    } else {
        key.char()
    }
}

impl Prompt {
    pub fn new(
        prompt: Cow<'static, str>,
        history_register: Option<char>,
        completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
        mut callback_fn: impl FnMut(&mut Context, &str, PromptEvent) + 'static,
    ) -> Self {
        Self::new_with_callback(
            prompt,
            history_register,
            completion_fn,
            move |cx, input, event| {
                callback_fn(cx, input, event);
                None
            },
        )
    }

    /// Creates a prompt whose validation may schedule work after the prompt closes.
    ///
    /// Callbacks returned for update or abort events are ignored.
    pub fn new_with_callback(
        prompt: Cow<'static, str>,
        history_register: Option<char>,
        completion_fn: impl FnMut(&Editor, &str) -> CompletionResult + 'static,
        callback_fn: impl FnMut(&mut Context, &str, PromptEvent) -> Option<crate::compositor::Callback>
            + 'static,
    ) -> Self {
        Self {
            session: PromptSession::default(),
            prompt,
            line: String::new(),
            cursor: 0,
            line_area: Rect::default(),
            anchor: 0,
            truncate_start: false,
            truncate_end: false,
            completion: Vec::new(),
            completion_line: String::new(),
            selection: None,
            history_register,
            history_pos: None,
            history_draft: None,
            history_edited: true,
            cached_suggestion: std::cell::OnceCell::new(),
            search_syntax: None,
            completion_fn: Box::new(completion_fn),
            task_controller: TaskController::new(),
            deferred_completion: None,
            pending_completion: None,
            pending_actions: None,
            pending_input: Vec::new(),
            action_scheduled: false,
            callback_fn: Box::new(callback_fn),
            doc_fn: Box::new(|_| None),
            next_char_handler: None,
            keys: KeymapState::default(),
            language: None,
        }
    }

    pub(super) fn with_search_syntax(mut self, syntax: Option<Rc<Cell<SearchSyntax>>>) -> Self {
        self.search_syntax = syntax;
        self
    }

    fn syntax(&self) -> SearchSyntax {
        self.search_syntax
            .as_ref()
            .map_or(SearchSyntax::Regex, |syntax| syntax.get())
    }

    fn label(&self) -> Cow<'_, str> {
        if self.search_syntax.is_some() {
            let syntax = match self.syntax() {
                SearchSyntax::Regex => "regex",
                SearchSyntax::Literal => "literal",
            };
            Cow::Owned(format!("{} [{syntax}]:", self.prompt.trim_end_matches(':')))
        } else {
            Cow::Borrowed(self.prompt.as_ref())
        }
    }

    fn history_entry(&self) -> SearchHistoryEntry {
        SearchHistoryEntry {
            text: self.line.clone(),
            syntax: self.syntax(),
        }
    }

    fn restore_history_entry(&mut self, entry: SearchHistoryEntry, editor: &Editor) {
        if let Some(syntax) = &self.search_syntax {
            syntax.set(entry.syntax);
        }
        self.set_line(entry.text, editor);
    }

    fn history_entries<'a>(
        &self,
        editor: &'a Editor,
        register: char,
    ) -> Box<dyn DoubleEndedIterator<Item = SearchHistoryEntry> + 'a> {
        if self.search_syntax.is_some() && !matches!(register, '#' | '.' | '%') {
            editor.registers.search_history(register)
        } else {
            Box::new(
                editor
                    .read_register(register)
                    .into_iter()
                    .flatten()
                    .map(|value| SearchHistoryEntry::regex(value.into_owned())),
            )
        }
    }

    fn first_history_entry(&self, editor: &Editor) -> Option<&SearchHistoryEntry> {
        self.cached_suggestion
            .get_or_init(|| {
                self.history_register.and_then(|register| {
                    if self.search_syntax.is_some() {
                        self.history_entries(editor, register).next()
                    } else {
                        editor
                            .first_register_value(register)
                            .map(|value| SearchHistoryEntry::regex(value.into_owned()))
                    }
                })
            })
            .as_ref()
    }

    pub(crate) fn session(&self) -> &PromptSession {
        &self.session
    }

    pub(super) fn cancel_input(&mut self) {
        self.keys.clear();
        self.next_char_handler = None;
        self.completion.clear();
        self.pending_actions = None;
        self.pending_input.clear();
    }

    /// Gets the byte index in the input representing the current cursor location.
    #[inline]
    pub(crate) fn position(&self) -> usize {
        self.cursor
    }

    pub fn with_line(mut self, line: String, editor: &Editor) -> Self {
        self.set_line(line, editor);
        self
    }

    pub fn set_line(&mut self, line: String, editor: &Editor) {
        let cursor = line.len();
        self.line = line;
        self.cursor = cursor;
        self.recalculate_completion(editor);
    }

    pub fn with_language(
        mut self,
        language: &'static str,
        loader: Arc<ArcSwap<syntax::Loader>>,
    ) -> Self {
        self.language = Some((language, loader));
        self
    }

    pub fn line(&self) -> &String {
        &self.line
    }

    pub(crate) fn set_prompt(&mut self, prompt: impl Into<Cow<'static, str>>) {
        self.prompt = prompt.into();
    }

    pub fn with_history_register(&mut self, history_register: Option<char>) -> &mut Self {
        self.history_register = history_register;
        self.cached_suggestion.take();
        self
    }

    pub(crate) fn history_register(&self) -> Option<char> {
        self.history_register
    }

    pub(crate) fn first_history_completion<'a>(
        &'a self,
        editor: &'a Editor,
    ) -> Option<Cow<'a, str>> {
        self.first_history_entry(editor)
            .map(|entry| Cow::Borrowed(entry.text.as_str()))
    }

    pub fn recalculate_completion(&mut self, editor: &Editor) {
        self.history_edited = true;
        self.cached_suggestion.take();
        self.clamp_view();
        self.exit_selection();

        self.task_controller.cancel();
        if let Some(task) = &mut self.deferred_completion {
            task.obsolete = true;
        }
        self.pending_completion = None;
        self.completion_line.clone_from(&self.line);

        if self.next_char_handler.is_some() {
            self.completion.clear();
        }

        if self.search_syntax.is_some() {
            let mut entries = self
                .history_register
                .map(|register| {
                    self.history_entries(editor, register)
                        .take(200)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            entries.retain(|entry| {
                entry.syntax == self.syntax() && entry.text.starts_with(&self.line)
            });
            entries.sort_by(|a, b| a.text.cmp(&b.text));
            entries.dedup();
            self.completion = entries
                .into_iter()
                .map(|entry| (0.., entry.text.into()))
                .collect();
            return;
        }

        match (self.completion_fn)(editor, &self.line) {
            CompletionResult::Immediate(completion) => self.completion = completion,
            CompletionResult::Deferred(compute) => {
                // Keep the menu visible while its replacement is pending.
                if self.deferred_completion.is_some() {
                    self.pending_completion = Some(compute);
                } else {
                    self.start_completion(editor, compute);
                }
            }
        }
    }

    fn start_completion(&mut self, editor: &Editor, compute: DeferredCompletion) {
        let handle = self.task_controller.restart();
        let (tx, results) = std::sync::mpsc::sync_channel(1);
        // Also wake the application if a worker panics and disconnects the receiver.
        let redraw = editor.redraw().on_drop();
        tokio::task::spawn_blocking(move || {
            let _redraw = redraw;
            // Drop the sender before waking the UI, including during unwinding.
            let sender = tx;
            let _ = sender.send(compute(&handle));
        });
        self.deferred_completion = Some(CompletionTask {
            results,
            obsolete: false,
        });
    }

    fn completion_pending(&self) -> bool {
        self.pending_completion.is_some()
            || self
                .deferred_completion
                .as_ref()
                .is_some_and(|task| !task.obsolete)
    }

    /// Collect the worker's report before reusing its slot. Obsolete results never
    /// replace current suggestions, including immediate results shown in the meantime.
    fn handle_deferred_completion(&mut self, editor: &Editor) {
        let Some(task) = &self.deferred_completion else {
            return;
        };
        match task.results.try_recv() {
            Ok(completion) if !task.obsolete => self.completion = completion,
            Err(TryRecvError::Disconnected) if !task.obsolete => self.completion.clear(),
            Ok(_) | Err(TryRecvError::Disconnected) => {}
            Err(TryRecvError::Empty) => return,
        }
        self.deferred_completion = None;
        if let Some(compute) = self.pending_completion.take() {
            self.start_completion(editor, compute);
        }
    }

    fn resume_pending_input(&mut self, cx: &mut Context) {
        if self.completion_pending() || self.pending_actions.is_none() || self.action_scheduled {
            return;
        }
        self.action_scheduled = true;
        let session = self.session.clone();
        cx.jobs
            .sender()
            .enqueue(crate::job::Callback::ui(move |compositor, cx| {
                let Some(prompt) = compositor.find_prompt(&session) else {
                    return;
                };
                prompt.action_scheduled = false;
                let Some((actions, playback)) = prompt.pending_actions.take() else {
                    return;
                };
                let input = std::mem::take(&mut prompt.pending_input);
                let previous = std::mem::replace(&mut cx.interaction.macro_replaying, playback);
                let result = prompt.execute_actions(&actions, cx);
                if let EventResult::Consumed(Some(callback)) = result {
                    callback(compositor, cx);
                }
                cx.interaction.macro_replaying = previous;
                compositor.replay_deferred(input, cx);
            }));
    }

    fn clamp_view(&mut self) {
        self.cursor = self
            .line
            .floor_char_boundary(self.cursor.min(self.line.len()));
        self.anchor = self.line.floor_char_boundary(self.anchor.min(self.cursor));
    }

    /// Compute the cursor position after applying movement
    /// Taken from: <https://github.com/wez/wezterm/blob/e0b62d07ca9bf8ce69a61e30a3c20e7abc48ce7e/termwiz/src/lineedit/mod.rs#L516-L611>
    fn eval_movement(&self, movement: Movement) -> usize {
        match movement {
            Movement::BackwardChar(rep) => {
                let mut position = self.cursor;
                for _ in 0..rep {
                    let mut cursor = GraphemeCursor::new(position, self.line.len(), false);
                    if let Ok(Some(pos)) = cursor.prev_boundary(&self.line, 0) {
                        position = pos;
                    } else {
                        break;
                    }
                }
                position
            }
            Movement::BackwardWord(rep) => {
                let char_indices: Vec<(usize, char)> = self.line.char_indices().collect();
                if char_indices.is_empty() {
                    return self.cursor;
                }
                let mut char_position = char_indices
                    .iter()
                    .position(|(idx, _)| *idx == self.cursor)
                    .unwrap_or(char_indices.len() - 1);

                for _ in 0..rep {
                    if char_position == 0 {
                        break;
                    }

                    let mut found = None;
                    for prev in (0..char_position - 1).rev() {
                        if is_word_sep(char_indices[prev].1) {
                            found = Some(prev + 1);
                            break;
                        }
                    }

                    char_position = found.unwrap_or(0);
                }
                char_indices[char_position].0
            }
            Movement::ForwardWord(rep) => {
                let char_indices: Vec<(usize, char)> = self.line.char_indices().collect();
                if char_indices.is_empty() {
                    return self.cursor;
                }
                let mut char_position = char_indices
                    .iter()
                    .position(|(idx, _)| *idx == self.cursor)
                    .unwrap_or(char_indices.len());

                for _ in 0..rep {
                    // Skip any non-whitespace characters
                    while char_position < char_indices.len()
                        && !is_word_sep(char_indices[char_position].1)
                    {
                        char_position += 1;
                    }

                    // Skip any whitespace characters
                    while char_position < char_indices.len()
                        && is_word_sep(char_indices[char_position].1)
                    {
                        char_position += 1;
                    }

                    // We are now on the start of the next word
                }
                char_indices
                    .get(char_position)
                    .map(|(i, _)| *i)
                    .unwrap_or_else(|| self.line.len())
            }
            Movement::ForwardChar(rep) => {
                let mut position = self.cursor;
                for _ in 0..rep {
                    let mut cursor = GraphemeCursor::new(position, self.line.len(), false);
                    if let Ok(Some(pos)) = cursor.next_boundary(&self.line, 0) {
                        position = pos;
                    } else {
                        break;
                    }
                }
                position
            }
            Movement::StartOfLine => 0,
            Movement::EndOfLine => self.line.len(),
            Movement::None => self.cursor,
        }
    }

    fn take_next_char_handler(&mut self) -> Option<PromptCharHandler> {
        let handler = self.next_char_handler.take()?;
        self.completion.clear();
        Some(handler)
    }

    pub fn insert_char(&mut self, c: char, cx: &Context) {
        if let Some(handler) = self.take_next_char_handler() {
            handler(self, c, cx);
            return;
        }

        self.line.insert(self.cursor, c);
        let mut cursor = GraphemeCursor::new(self.cursor, self.line.len(), false);
        if let Ok(Some(pos)) = cursor.next_boundary(&self.line, 0) {
            self.cursor = pos;
        }
        self.recalculate_completion(cx.editor);
    }

    pub fn insert_str(&mut self, s: &str, editor: &Editor) {
        self.line.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.recalculate_completion(editor);
    }

    pub fn move_cursor(&mut self, movement: Movement) {
        let pos = self.eval_movement(movement);
        self.cursor = pos
    }

    pub fn move_start(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.line.len();
    }

    fn delete(&mut self, cx: &mut Context, delete: fn(&mut Self, &Editor)) {
        delete(self, cx.editor);
        (self.callback_fn)(cx, &self.line, PromptEvent::Update);
    }

    pub fn delete_char_backwards(&mut self, editor: &Editor) {
        let pos = self.eval_movement(Movement::BackwardChar(1));
        self.line.replace_range(pos..self.cursor, "");
        self.cursor = pos;

        self.recalculate_completion(editor);
    }

    pub fn delete_char_forwards(&mut self, editor: &Editor) {
        let pos = self.eval_movement(Movement::ForwardChar(1));
        self.line.replace_range(self.cursor..pos, "");

        self.recalculate_completion(editor);
    }

    pub fn delete_word_backwards(&mut self, editor: &Editor) {
        let pos = self.eval_movement(Movement::BackwardWord(1));
        self.line.replace_range(pos..self.cursor, "");
        self.cursor = pos;

        self.recalculate_completion(editor);
    }

    pub fn delete_word_forwards(&mut self, editor: &Editor) {
        let pos = self.eval_movement(Movement::ForwardWord(1));
        self.line.replace_range(self.cursor..pos, "");

        self.recalculate_completion(editor);
    }

    pub fn kill_to_start_of_line(&mut self, editor: &Editor) {
        let pos = self.eval_movement(Movement::StartOfLine);
        self.line.replace_range(pos..self.cursor, "");
        self.cursor = pos;

        self.recalculate_completion(editor);
    }

    pub fn kill_to_end_of_line(&mut self, editor: &Editor) {
        let pos = self.eval_movement(Movement::EndOfLine);
        self.line.replace_range(self.cursor..pos, "");

        self.recalculate_completion(editor);
    }

    pub fn clear(&mut self, editor: &Editor) {
        self.line.clear();
        self.cursor = 0;
        self.recalculate_completion(editor);
    }

    pub fn change_history(
        &mut self,
        cx: &mut Context,
        register: char,
        direction: CompletionDirection,
    ) {
        (self.callback_fn)(cx, &self.line, PromptEvent::Abort);
        if self.history_edited {
            self.history_pos = None;
            self.history_draft = Some(self.history_entry());
        }
        if self.history_draft.is_none() {
            self.history_draft = Some(self.history_entry());
        }
        let draft = &self.history_draft.as_ref().unwrap().text;
        let values: Vec<_> = self
            .history_entries(cx.editor, register)
            .rev()
            .filter(|entry| entry.text.contains(draft))
            .collect();
        if values.is_empty() {
            return;
        }
        let newest = values.len() - 1;
        if matches!(direction, CompletionDirection::Forward) && self.history_pos == Some(newest) {
            let entry = self.history_draft.take().unwrap();
            self.line = entry.text;
            if let Some(syntax) = &self.search_syntax {
                syntax.set(entry.syntax);
            }
            self.history_pos = None;
        } else {
            let index = match direction {
                CompletionDirection::Forward => self.history_pos.map_or(0, |index| index + 1),
                CompletionDirection::Backward => {
                    self.history_pos.unwrap_or(values.len()).saturating_sub(1)
                }
            }
            .min(newest);
            self.line.clone_from(&values[index].text);
            if let Some(syntax) = &self.search_syntax {
                syntax.set(values[index].syntax);
            }
            self.history_pos = Some(index);
        }
        self.move_end();
        self.anchor = 0;
        (self.callback_fn)(cx, &self.line, PromptEvent::Update);
        self.recalculate_completion(cx.editor);
        self.history_edited = false;
    }

    pub fn change_completion_selection(&mut self, direction: CompletionDirection) {
        if self.completion_pending() || self.completion.is_empty() {
            return;
        }

        let index = match direction {
            CompletionDirection::Forward => self.selection.map_or(0, |i| i + 1),
            CompletionDirection::Backward => {
                self.selection.unwrap_or(0) + self.completion.len() - 1
            }
        } % self.completion.len();

        self.history_edited = true;
        self.selection = Some(index);

        let (range, item) = &self.completion[index];

        self.line.clone_from(&self.completion_line);
        self.line.replace_range(range.clone(), &item.content);

        self.move_end();
        self.clamp_view();
    }

    pub fn exit_selection(&mut self) {
        self.selection = None;
    }
}

const BASE_WIDTH: u16 = 30;

impl Prompt {
    pub fn render_prompt(&mut self, area: Rect, surface: &mut Surface, cx: &mut DrawContext) {
        if area.is_empty() {
            self.line_area = area;
            return;
        }
        let theme = &cx.editor.theme;
        let prompt_color = theme.get("ui.text");
        let completion_color = theme.get("ui.menu");
        let selected_color = theme.get("ui.menu.selected");
        let suggestion_color = theme.get("ui.text.inactive");
        let background = theme.get("ui.background");
        // completion

        let max_len = self
            .completion
            .iter()
            .map(|(_, completion)| completion.content.len() as u16)
            .max()
            .unwrap_or(BASE_WIDTH)
            .max(BASE_WIDTH);

        let cols = std::cmp::max(1, area.width / max_len);
        let col_width = (area.width.saturating_sub(cols)) / cols;

        let height = (self.completion.len() as u16)
            .div_ceil(cols)
            .min(10) // at most 10 rows (or less)
            .min(area.height.saturating_sub(1));

        let completion_area = Rect::new(
            area.x,
            area.bottom().saturating_sub(height.saturating_add(1)),
            area.width,
            height,
        );

        if completion_area.height > 0 && !self.completion.is_empty() {
            let area = completion_area;
            let background = theme.get("ui.menu");

            let items = height as usize * cols as usize;

            let offset = self
                .selection
                .map(|selection| selection / items * items)
                .unwrap_or_default();

            cx.hyperlinks.borrow_mut().clear(area);
            surface.clear_with(area, background);
            // PERF: Ratatui tables require temporary row/cell vectors instead of writing the
            // completion grid directly. Cache this page if prompt rendering becomes measurable.
            let rows = (0..height).map(|row| {
                let cells = (0..cols).map(|col| {
                    let index = offset + (col * height + row) as usize;
                    let Some((_range, completion)) = self.completion.get(index) else {
                        return Span::raw("");
                    };
                    let style = if Some(index) == self.selection {
                        selected_color
                    } else {
                        completion_color.patch(completion.style)
                    };
                    Span::styled(completion.content.as_ref(), style)
                });
                Row::new(cells)
            });
            let widths = vec![Constraint::Length(col_width.saturating_sub(1)); cols as usize];
            let mut state = TableState::default();
            let table = Table::new(rows, &widths)
                .style(background)
                .column_spacing(2)
                .flex(tui::layout::Flex::Legacy);
            StatefulWidget::render(table, area, surface, &mut state);
        }

        if let Some(doc) = (self.doc_fn)(&self.line) {
            // PERF: Command documentation is small, so parsing it on each render keeps this
            // integration simple. Cache the parsed Markdown if profiling shows this is measurable.
            let markdown = ui::Markdown::new(doc.into_owned(), cx.editor.syn_loader.clone());
            let text = markdown.parse(Some(&cx.editor.theme));
            let paragraph = ui::text::paragraph(text);

            let max_width = BASE_WIDTH * 3;
            let text_width = max_width - ui::panel::PADDED_HORIZONTAL_INSET;

            let viewport = area;

            let (_width, height) = ui::text::required_size(&paragraph, text_width);

            let area = viewport.intersection(Rect::new(
                completion_area.x,
                completion_area
                    .y
                    .saturating_sub(height.saturating_add(ui::panel::BORDER_INSET)),
                max_width,
                height.saturating_add(ui::panel::BORDER_INSET),
            ));

            let background = theme.get("ui.help");
            cx.hyperlinks.borrow_mut().clear(area);
            surface.clear_with(area, background);

            let block = ui::panel::horizontally_padded(
                theme,
                ui::PopupBorder::CommandLine.resolve(&cx.editor.config()),
            );
            let inner = block.inner(area);

            block.render(area, surface);
            paragraph.render(inner, surface);
        }

        let line = area.height - 1;
        cx.hyperlinks.borrow_mut().clear(area.clip_top(line));
        surface.clear_with(area.clip_top(line), background);
        // render buffer text
        let label = self.label();
        surface.set_string(area.x, area.y + line, &label, prompt_color);

        self.line_area = area
            .clip_left(label.width() as u16)
            .clip_top(line)
            .clip_right(2);
        if self.line_area.is_empty() {
            return;
        }

        self.clamp_view();

        if self.line.is_empty() {
            self.anchor = 0;
            // Show the most recently entered value as a suggestion.
            if let Some(suggestion) = self.first_history_completion(cx.editor) {
                surface.set_string(
                    self.line_area.x,
                    self.line_area.y,
                    &suggestion,
                    suggestion_color,
                );
            }
        } else if let Some((language, loader)) = self
            .language
            .as_ref()
            .filter(|_| self.syntax() == SearchSyntax::Regex)
        {
            let mut text: ui::text::Text = crate::ui::markdown::highlighted_code_block(
                &self.line,
                language,
                Some(&cx.editor.theme),
                &loader.load(),
                None,
            )
            .into();
            text.render(self.line_area, surface, cx);
        } else {
            let line_width = self.line_area.width as usize;

            if self.line.width() < line_width {
                self.anchor = 0;
            } else if self.cursor <= self.anchor {
                // Ensure the grapheme under the cursor is in view.
                self.anchor = self.line[..self.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or_default();
            } else if self.line[self.anchor..self.cursor].width() > line_width {
                // Set the anchor to the last grapheme cluster before the width is exceeded.
                let mut width = 0;
                self.anchor = self.line[..self.cursor]
                    .grapheme_indices(true)
                    .rev()
                    .find_map(|(idx, g)| {
                        width += g.width();
                        if width > line_width {
                            Some(idx + g.len())
                        } else {
                            None
                        }
                    })
                    .unwrap();
            }

            // Fill space exposed by deleting text or widening the terminal.
            if self.anchor > 0 && self.line[self.anchor..].width() < line_width {
                let mut width = 0;
                self.anchor = self
                    .line
                    .grapheme_indices(true)
                    .rev()
                    .find_map(|(idx, g)| {
                        width += g.width();
                        (width > line_width).then_some(idx + g.len())
                    })
                    .unwrap_or_default();
            }

            self.truncate_start = self.anchor > 0;
            self.truncate_end = self.line[self.anchor..].width() > line_width;

            // if we keep inserting characters just before the end elipsis, we move the anchor
            // so that those new characters are displayed
            if self.truncate_end && self.line[self.anchor..self.cursor].width() >= line_width {
                // Move the anchor forward by one non-zero-width grapheme.
                self.anchor += self.line[self.anchor..]
                    .grapheme_indices(true)
                    .find_map(|(idx, g)| {
                        if g.width() > 0 {
                            Some(idx + g.len())
                        } else {
                            None
                        }
                    })
                    .unwrap();
            }

            surface.set_string_anchored(
                self.line_area.x,
                self.line_area.y,
                self.truncate_start,
                self.truncate_end,
                &self.line.as_str()[self.anchor..],
                line_width,
                |_| prompt_color,
            );
        }
    }

    fn execute_actions(&mut self, actions: &[PromptAction], cx: &mut Context) -> EventResult {
        let close_fn = || {
            EventResult::Consumed(Some(Box::new(|compositor: &mut Compositor, _| {
                compositor.pop();
            })))
        };

        for (index, &action) in actions.iter().enumerate() {
            if self.completion_pending()
                && matches!(
                    action,
                    PromptAction::CompleteNext | PromptAction::CompletePrevious
                )
            {
                self.pending_actions = Some((
                    actions[index..].to_vec(),
                    cx.interaction.macro_replaying.clone(),
                ));
                return EventResult::Consumed(None);
            }
            match action {
                PromptAction::Cancel => {
                    (self.callback_fn)(cx, &self.line, PromptEvent::Abort);
                    return close_fn();
                }
                PromptAction::MoveWordLeft => self.move_cursor(Movement::BackwardWord(1)),
                PromptAction::MoveWordRight => self.move_cursor(Movement::ForwardWord(1)),
                PromptAction::MoveLeft => self.move_cursor(Movement::BackwardChar(1)),
                PromptAction::MoveRight => {
                    if self.line.is_empty()
                        && let Some(entry) = self.first_history_entry(cx.editor).cloned()
                    {
                        self.restore_history_entry(entry, cx.editor);
                        (self.callback_fn)(cx, &self.line, PromptEvent::Update);
                    } else {
                        self.move_cursor(Movement::ForwardChar(1));
                    }
                }
                PromptAction::MoveEnd => self.move_end(),
                PromptAction::MoveStart => self.move_start(),
                PromptAction::DeleteWordBackward => self.delete(cx, Self::delete_word_backwards),
                PromptAction::DeleteWordForward => self.delete(cx, Self::delete_word_forwards),
                PromptAction::DeleteToEnd => self.delete(cx, Self::kill_to_end_of_line),
                PromptAction::DeleteToStart => self.delete(cx, Self::kill_to_start_of_line),
                PromptAction::DeleteBackward => self.delete(cx, Self::delete_char_backwards),
                PromptAction::DeleteForward => self.delete(cx, Self::delete_char_forwards),
                PromptAction::InsertWord => {
                    let (view, doc) = current!(cx.editor);
                    let text = doc.text().slice(..);

                    use editor_core::textobject;
                    let range = textobject::textobject_word(
                        text,
                        doc.selection(view.id).primary(),
                        textobject::TextObject::Inside,
                        1,
                        false,
                    );
                    let line = text.slice_chars(range.from()..range.to()).to_string();
                    if !line.is_empty() {
                        self.insert_str(line.as_str(), cx.editor);
                        (self.callback_fn)(cx, &self.line, PromptEvent::Update);
                    }
                }
                PromptAction::Accept => {
                    if self.selection.is_some() && self.line.ends_with(std::path::MAIN_SEPARATOR) {
                        self.recalculate_completion(cx.editor);
                    } else {
                        let last_entry = self.first_history_entry(cx.editor).cloned();
                        if self.line.is_empty() {
                            if let Some(entry) = last_entry {
                                self.line = entry.text;
                                self.cursor = self.line.len();
                                if let Some(syntax) = &self.search_syntax {
                                    syntax.set(entry.syntax);
                                }
                            }
                        } else if self.search_syntax.is_none()
                            && last_entry
                                .as_ref()
                                .is_none_or(|entry| entry.text != self.line)
                            && let Some(register) = self.history_register
                            && let Err(err) =
                                cx.editor.registers.push_unique(register, self.line.clone())
                        {
                            cx.editor.set_error(|| err.to_string());
                        }

                        let callback = (self.callback_fn)(cx, &self.line, PromptEvent::Validate);

                        return match callback {
                            Some(callback) => EventResult::Consumed(Some(Box::new(
                                move |compositor: &mut Compositor, cx| {
                                    compositor.pop();
                                    callback(compositor, cx);
                                },
                            ))),
                            None => close_fn(),
                        };
                    }
                }
                PromptAction::HistoryPrevious => {
                    if let Some(register) = self.history_register {
                        self.change_history(cx, register, CompletionDirection::Backward);
                    }
                }
                PromptAction::HistoryNext => {
                    if let Some(register) = self.history_register {
                        self.change_history(cx, register, CompletionDirection::Forward);
                    }
                }
                PromptAction::CompleteNext => {
                    self.change_completion_selection(CompletionDirection::Forward);
                    // if single completion candidate is a directory list content in completion
                    if self.completion.len() == 1 && self.line.ends_with(std::path::MAIN_SEPARATOR)
                    {
                        self.recalculate_completion(cx.editor);
                    }
                    (self.callback_fn)(cx, &self.line, PromptEvent::Update);
                }
                PromptAction::CompletePrevious => {
                    self.change_completion_selection(CompletionDirection::Backward);
                    (self.callback_fn)(cx, &self.line, PromptEvent::Update);
                }
                PromptAction::ExitSelection => self.exit_selection(),
                PromptAction::ToggleSearchSyntax => {
                    if let Some(syntax) = &self.search_syntax {
                        syntax.set(match syntax.get() {
                            SearchSyntax::Regex => SearchSyntax::Literal,
                            SearchSyntax::Literal => SearchSyntax::Regex,
                        });
                        self.recalculate_completion(cx.editor);
                        (self.callback_fn)(cx, &self.line, PromptEvent::Update);
                    }
                }
                PromptAction::InsertRegister => {
                    self.completion = cx
                        .editor
                        .registers
                        .iter_preview()
                        .map(|(ch, preview)| (0.., format!("{} {}", ch, preview).into()))
                        .collect();
                    self.next_char_handler = Some(Box::new(|prompt, c, context| {
                        prompt.insert_str(
                            &context.editor.first_register_value(c).unwrap_or_default(),
                            context.editor,
                        );
                    }));
                    (self.callback_fn)(cx, &self.line, PromptEvent::Update);
                    return EventResult::Consumed(None);
                }
            }
        }

        EventResult::Consumed(None)
    }
}

impl Component for Prompt {
    /// Collect completions computed on a background thread; the task producing them requested
    /// the redraw this update runs in.
    fn update(&mut self, _area: Rect, cx: &mut Context) {
        self.handle_deferred_completion(cx.editor);
        self.resume_pending_input(cx);
    }

    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending() || self.next_char_handler.is_some() || self.pending_actions.is_some()
    }

    /// Completions computed on a background thread change the menu once they arrive.
    fn has_pending_render(&self) -> bool {
        self.completion_pending() || self.pending_actions.is_some()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if self.pending_actions.is_some() {
            // Cancellation must remain available even when a filesystem call is stuck.
            if cx.interaction.macro_replaying.is_empty()
                && let Event::Key(key) = event
                && let Lookup::Actions([PromptAction::Cancel]) =
                    KeymapState::default().lookup(&cx.config.current.component_keys.prompt, *key)
            {
                self.pending_actions = None;
                self.pending_input.clear();
                return self.execute_actions(&[PromptAction::Cancel], cx);
            }
            self.pending_input
                .push(DeferredEvent::capture(event, cx.interaction));
            return EventResult::Consumed(None);
        }
        let event = match event {
            Event::Paste(data) => {
                self.keys.clear();
                self.take_next_char_handler();
                self.insert_str(data, cx.editor);
                (self.callback_fn)(cx, &self.line, PromptEvent::Update);
                return EventResult::Consumed(None);
            }
            Event::Key(event) => *event,
            Event::Resize(..) => return EventResult::Consumed(None),
            // Prompt is a modal and should consume mouse events so clicks don't fall
            // through to the editor underneath
            Event::Mouse(_) => return EventResult::Consumed(None),
            _ => return EventResult::Ignored(None),
        };

        // Register selection reads a literal character before consulting bindings.
        if self.next_char_handler.is_some()
            && event.code == KeyCode::Esc
            && event.modifiers.is_empty()
        {
            self.take_next_char_handler();
            self.recalculate_completion(cx.editor);
            return EventResult::Consumed(None);
        }
        if self.next_char_handler.is_some()
            && let Some(c) = input_char(event)
        {
            self.insert_char(c, cx);
            (self.callback_fn)(cx, &self.line, PromptEvent::Update);
            return EventResult::Consumed(None);
        }
        let actions = match self
            .keys
            .lookup(&cx.config.current.component_keys.prompt, event)
        {
            Lookup::Actions(actions) => actions,
            Lookup::Pending => return EventResult::Consumed(None),
            Lookup::Unbound => {
                if let Some(c) = input_char(event) {
                    self.insert_char(c, cx);
                    (self.callback_fn)(cx, &self.line, PromptEvent::Update);
                }
                return EventResult::Consumed(None);
            }
        };

        self.execute_actions(actions, cx)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut DrawContext) {
        self.render_prompt(area, surface, cx)
    }

    fn owns_cursor(&self) -> bool {
        true
    }

    fn cursor(&self, area: Rect, cx: &DrawContext) -> Cursor {
        let editor = cx.editor;
        if area.is_empty() || self.line_area.is_empty() {
            return Cursor::Hidden;
        }
        let area = area
            .clip_left(self.label().width() as u16)
            .clip_right(if self.prompt.is_empty() { 2 } else { 0 });

        let cursor = self
            .line
            .floor_char_boundary(self.cursor.min(self.line.len()));
        let anchor = self.line.floor_char_boundary(self.anchor.min(cursor));
        let mut col = area.left() as usize + self.line[anchor..cursor].width();

        // ensure the cursor does not go beyond elipses
        if self.truncate_end && self.line[anchor..cursor].width() >= self.line_area.width as usize {
            col = col.saturating_sub(1);
        }

        if self.truncate_start && cursor == anchor {
            col += self.line[cursor..]
                .graphemes(true)
                .next()
                .map_or(0, |g| g.width());
        }

        let line = area.height as usize - 1;

        Cursor::Native(
            (col as u16, area.y + line as u16).into(),
            Mode::Insert.cursor_shape(&editor.config().cursor.shape),
        )
    }
}
use tui::buffer::BufferExt as _;
use view::graphics::RectExt as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortened_prompt_clamps_scroll_and_cursor_to_character_boundaries() {
        let mut prompt = Prompt::new(
            ":".into(),
            None,
            |_, _| CompletionResult::Immediate(Vec::new()),
            |_, _, _| {},
        );
        prompt.anchor = 30;
        prompt.cursor = 40;
        prompt.clamp_view();
        assert_eq!((prompt.anchor, prompt.cursor), (0, 0));

        prompt.line = "界界".into();
        prompt.anchor = 4;
        prompt.cursor = 6;
        prompt.clamp_view();
        assert_eq!((prompt.anchor, prompt.cursor), (3, 6));
    }
}
