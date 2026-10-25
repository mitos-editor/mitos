use crate::keymap::component::{ComponentKeymaps, PickerAction, PickerItemAction};
use ui_core::keymap::{Keymap, KeymapState, Lookup};
mod file_updates;
mod handlers;
#[cfg(all(test, feature = "integration"))]
mod matching_tests;
mod preview;
mod query;
mod replacement;
mod results;
#[cfg(all(test, feature = "integration"))]
pub(crate) mod test;
mod updates;

use crate::{
    compositor::{self, Component, Compositor, Context, Cursor, Event, EventResult},
    ui::{self, panel, picker::query::PickerQuery},
};
use event::AsyncHook;
use futures_util::future::BoxFuture;
use nucleo::{Config, Nucleo};
use thiserror::Error;
use tokio::sync::mpsc::Sender;
use tui::{
    buffer::Buffer as Surface,
    layout::{Constraint, Layout, Size},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
};

use tui::buffer::BufferExt as _;
use tui::widgets::Widget;
use view::graphics::RectExt as _;

use std::{
    borrow::Cow,
    sync::{
        atomic::{self, AtomicUsize},
        Arc,
    },
};

use crate::ui::{Prompt, PromptEvent};
use editor_core::{movement::Direction, unicode::segmentation::UnicodeSegmentation};
use view::{
    editor::Action,
    graphics::{Modifier, Rect},
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Editor,
};

use self::handlers::{DynamicQueryChange, DynamicQueryHandler};
use self::preview::{render_preview_content, FilePreview};
use self::replacement::{split_picker_input_area, CaseSensitiveToggle, Replacement};

// These types remain available through the picker facade.
pub use self::preview::{
    CachedPreview, FileLocation, ImagePreview, PathOrId, Preview, MAX_FILE_SIZE_FOR_PREVIEW,
};
pub(crate) use updates::UpdateContext;

type FileCallback<T> = Box<dyn for<'a> Fn(&'a Editor, &'a T) -> Option<FileLocation<'a>>>;
type QuicklistCallback<T> = Box<dyn Fn(&Editor, &T) -> Option<QuicklistEntry>>;

/// A native preview surface independent of file loading and repository providers.
/// Implementations own their background work and cancel it when hidden or dropped.
pub trait PickerPreview<T> {
    fn render(&mut self, item: Option<&T>, area: Rect, surface: &mut Surface, cx: &mut Context);
    fn cancel(&mut self);
    fn open_on_accept(&self, _item: &T) -> bool {
        false
    }
    /// A preview that opens an editor document follows the normal picker close
    /// path, including retaining it for `last_picker`.
    fn close_on_open(&self) -> bool {
        false
    }
    fn action(
        &mut self,
        _item: Option<&T>,
        _action: PreviewAction,
        _cx: &mut Context,
    ) -> EventResult {
        EventResult::Consumed(None)
    }
    fn invalidate(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Copy)]
pub enum PreviewAction {
    ScrollUp,
    ScrollDown,
    Previous,
    Next,
    Open(Action),
}

pub const ID: &str = "picker";

pub const MIN_AREA_WIDTH_FOR_PREVIEW: u16 = 72;

fn split_picker_area(area: Rect, show_preview: bool) -> (Rect, Option<Rect>) {
    if show_preview {
        let [picker, preview] =
            Layout::horizontal([Constraint::Length(area.width / 2), Constraint::Min(0)])
                .areas(area);
        (picker, Some(preview))
    } else {
        (area, None)
    }
}

fn inject_nucleo_item<T, D>(
    injector: &nucleo::Injector<T>,
    columns: &[Column<T, D>],
    item: T,
    editor_data: &D,
) {
    injector.push(item, |item, dst| {
        for (column, text) in columns.iter().filter(|column| column.filter).zip(dst) {
            *text = column.format_text(item, editor_data).into()
        }
    });
}

pub struct Injector<T, D> {
    dst: nucleo::Injector<T>,
    columns: Arc<[Column<T, D>]>,
    editor_data: Arc<D>,
    version: usize,
    picker_version: Arc<AtomicUsize>,
    /// A marker that requests a redraw when the injector drops.
    /// This marker causes the "running" indicator to disappear when a background job
    /// providing items is finished and drops. This could be wrapped in an [Arc] to ensure
    /// that the redraw is only requested when all Injectors drop for a Picker (which removes
    /// the "running" indicator) but the redraw handle is debounced so this is unnecessary.
    redraw: event::Redraw,
    _redraw: event::RequestRedrawOnDrop,
}

impl<I, D> Clone for Injector<I, D> {
    fn clone(&self) -> Self {
        Injector {
            dst: self.dst.clone(),
            columns: self.columns.clone(),
            editor_data: self.editor_data.clone(),
            version: self.version,
            picker_version: self.picker_version.clone(),
            redraw: self.redraw.clone(),
            _redraw: self._redraw.clone(),
        }
    }
}

#[derive(Error, Debug)]
#[error("picker has been shut down")]
pub struct InjectorShutdown;

impl<T, D> Injector<T, D> {
    /// Whether a newer query or a closed picker has invalidated this injector.
    pub(crate) fn is_shutdown(&self) -> bool {
        self.version != self.picker_version.load(atomic::Ordering::Relaxed)
    }

    pub fn push(&self, item: T) -> Result<(), InjectorShutdown> {
        if self.is_shutdown() {
            return Err(InjectorShutdown);
        }

        inject_nucleo_item(&self.dst, &self.columns, item, &self.editor_data);
        Ok(())
    }
}

type ColumnFormatFn<T, D> = for<'a> fn(&'a T, &'a D) -> Cell<'a>;
type ColumnPrefixFn<T, D> = for<'a> fn(&'a T, &'a D) -> Option<Span<'a>>;

pub struct Column<T, D> {
    name: Arc<str>,
    format: ColumnFormatFn<T, D>,
    prefix: ColumnPrefixFn<T, D>,
    /// Whether the column should be passed to nucleo for matching and filtering.
    /// `DynamicPicker` uses this so that the dynamic column (for example regex in
    /// global search) is not used for filtering twice.
    filter: bool,
    hidden: bool,
}

impl<T, D> Column<T, D> {
    pub fn new(name: impl Into<Arc<str>>, format: ColumnFormatFn<T, D>) -> Self {
        Self {
            name: name.into(),
            format,
            prefix: |_, _| None,
            filter: true,
            hidden: false,
        }
    }

    /// A column which does not display any contents
    pub fn hidden(name: impl Into<Arc<str>>) -> Self {
        let format = |_: &T, _: &D| unreachable!();

        Self {
            name: name.into(),
            format,
            prefix: |_, _| None,
            filter: false,
            hidden: true,
        }
    }

    pub fn without_filtering(mut self) -> Self {
        self.filter = false;
        self
    }

    /// Add a decorative prefix without including it in fuzzy matching or highlights.
    pub fn with_prefix(mut self, prefix: ColumnPrefixFn<T, D>) -> Self {
        self.prefix = prefix;
        self
    }

    fn format<'a>(&self, item: &'a T, data: &'a D) -> Cell<'a> {
        (self.format)(item, data)
    }

    fn format_text<'a>(&self, item: &'a T, data: &'a D) -> Cow<'a, str> {
        let text = self.format(item, data).content.to_string();
        text.into()
    }
}

fn visible_column_widths<T, D>(columns: &[Column<T, D>]) -> Vec<Constraint> {
    columns
        .iter()
        .filter(|column| !column.hidden)
        .map(|column| Constraint::Length(column.name.chars().count() as u16))
        .collect()
}

/// Returns a new list of options to replace the contents of the picker
/// when called with the current picker query,
type DynQueryCallback<T, D> =
    fn(&str, &mut Editor, Arc<D>, &Injector<T, D>) -> BoxFuture<'static, anyhow::Result<()>>;

pub struct Picker<T: 'static + Send + Sync, D: 'static> {
    keys: KeymapState,
    keymap: fn(&ComponentKeymaps) -> &Keymap<PickerAction>,
    item_action_fn: Option<PickerItemCallback<T, D>>,
    help: Option<fn(&ComponentKeymaps) -> String>,
    columns: Arc<[Column<T, D>]>,
    results: results::Results<T, D>,
    primary_column: usize,
    highlight_matcher: fuzzy::Matcher,
    redraw: event::Redraw,

    /// Current height of the completions box
    completion_height: u16,

    prompt: Prompt,
    query: PickerQuery,

    /// Whether to show the preview panel (default true)
    show_preview: bool,
    /// Constraints for tabular formatting
    widths: Vec<Constraint>,

    callback_fn: PickerCallback<T, D>,
    default_action: Action,

    pub truncate_start: bool,
    preview: FilePreview,
    custom_preview: Option<Box<dyn PickerPreview<T>>>,
    /// Given an item in the picker, return the file path and line number to display.
    file_fn: Option<FileCallback<T>>,
    /// Given an item in the picker, return an explicit quicklist entry.
    quicklist_fn: Option<QuicklistCallback<T>>,
    replacement: Option<Replacement<T, D>>,
    case_sensitive: Option<CaseSensitiveToggle<D>>,
    dynamic_query_pending: bool,
    dynamic_query_handler: Option<Sender<DynamicQueryChange>>,
    dynamic_query_request: event::TaskController,
    poll: Option<PollTask>,
    file_updates: Option<file_updates::FileUpdates<T, D>>,
    editor_updates: Option<updates::EditorUpdates<T>>,
    query_updates: Option<updates::QueryUpdates<D>>,
    query_refresh: bool,
    query_needs_refresh: bool,
    title: Option<Line<'static>>,
}

/// A closed or dropped picker must not keep waking its frontend.
struct PollTask {
    task: tokio::task::JoinHandle<()>,
    interval: std::time::Duration,
}

impl Drop for PollTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> Picker<T, D> {
    pub fn stream(
        redraw: event::Redraw,
        columns: impl IntoIterator<Item = Column<T, D>>,
        editor_data: D,
    ) -> (Nucleo<T>, Injector<T, D>) {
        let columns: Arc<[_]> = columns.into_iter().collect();
        let matcher_columns = columns.iter().filter(|col| col.filter).count() as u32;
        assert!(matcher_columns > 0);
        let matcher = Nucleo::new(
            Config::DEFAULT,
            Arc::new(redraw.callback()),
            None,
            matcher_columns,
        );
        let streamer = Injector {
            dst: matcher.injector(),
            columns,
            editor_data: Arc::new(editor_data),
            version: 0,
            picker_version: Arc::new(AtomicUsize::new(0)),
            redraw: redraw.clone(),
            _redraw: redraw.on_drop(),
        };
        (matcher, streamer)
    }

    pub fn new<C, O, F>(
        redraw: event::Redraw,
        columns: C,
        primary_column: usize,
        options: O,
        editor_data: D,
        callback_fn: F,
    ) -> Self
    where
        C: IntoIterator<Item = Column<T, D>>,
        O: IntoIterator<Item = T>,
        F: Fn(&mut Context, &T, Action) + 'static,
    {
        Self::new_with_callback_result(
            redraw,
            columns,
            primary_column,
            options,
            editor_data,
            move |cx, item, action| {
                callback_fn(cx, item, action);
                PickerCallbackResult::Close
            },
        )
    }

    pub(crate) fn new_with_callback_result<C, O, F>(
        redraw: event::Redraw,
        columns: C,
        primary_column: usize,
        options: O,
        editor_data: D,
        callback_fn: F,
    ) -> Self
    where
        C: IntoIterator<Item = Column<T, D>>,
        O: IntoIterator<Item = T>,
        F: Fn(&mut Context, &T, Action) -> PickerCallbackResult<T, D> + 'static,
    {
        let columns: Arc<[_]> = columns.into_iter().collect();
        let matcher_columns = columns
            .iter()
            .filter(|col: &&Column<T, D>| col.filter)
            .count() as u32;
        assert!(matcher_columns > 0);
        let matcher = Nucleo::new(
            Config::DEFAULT,
            Arc::new(redraw.callback()),
            None,
            matcher_columns,
        );
        let injector = matcher.injector();
        for item in options {
            inject_nucleo_item(&injector, &columns, item, &editor_data);
        }
        Self::with(
            redraw,
            matcher,
            columns,
            primary_column,
            Arc::new(editor_data),
            Arc::new(AtomicUsize::new(0)),
            callback_fn,
        )
    }

    pub fn with_stream(
        matcher: Nucleo<T>,
        primary_column: usize,
        injector: Injector<T, D>,
        callback_fn: impl Fn(&mut Context, &T, Action) + 'static,
    ) -> Self {
        Self::with(
            injector.redraw.clone(),
            matcher,
            injector.columns,
            primary_column,
            injector.editor_data,
            injector.picker_version,
            move |cx, item, action| {
                callback_fn(cx, item, action);
                PickerCallbackResult::Close
            },
        )
    }

    fn with(
        redraw: event::Redraw,
        matcher: Nucleo<T>,
        columns: Arc<[Column<T, D>]>,
        default_column: usize,
        editor_data: Arc<D>,
        version: Arc<AtomicUsize>,
        callback_fn: impl Fn(&mut Context, &T, Action) -> PickerCallbackResult<T, D> + 'static,
    ) -> Self {
        assert!(!columns.is_empty());

        let prompt = Prompt::new(
            "".into(),
            None,
            ui::completers::none,
            |_editor: &mut Context, _pattern: &str, _event: PromptEvent| {},
        );

        let widths = visible_column_widths(&columns);

        let query = PickerQuery::new(columns.iter().map(|col| &col.name).cloned(), default_column);

        Self {
            columns,
            primary_column: default_column,
            results: results::Results::new(matcher, editor_data, version),
            highlight_matcher: fuzzy::Matcher::default(),
            redraw,
            keys: KeymapState::default(),
            keymap: |maps| &maps.picker,
            item_action_fn: None,
            help: None,
            prompt,
            query,
            truncate_start: true,
            show_preview: true,
            callback_fn: Box::new(callback_fn),
            default_action: Action::Replace,
            completion_height: 0,
            widths,
            preview: FilePreview::default(),
            custom_preview: None,
            file_fn: None,
            quicklist_fn: None,
            replacement: None,
            case_sensitive: None,
            dynamic_query_pending: false,
            dynamic_query_handler: None,
            dynamic_query_request: event::TaskController::new(),
            poll: None,
            file_updates: None,
            editor_updates: None,
            query_updates: None,
            query_refresh: false,
            query_needs_refresh: false,
            title: None,
        }
    }

    pub fn injector(&self) -> Injector<T, D> {
        Injector {
            dst: self.results.matcher.injector(),
            columns: self.columns.clone(),
            editor_data: self.results.editor_data.clone(),
            version: self.results.version.load(atomic::Ordering::Relaxed),
            picker_version: self.results.version.clone(),
            redraw: self.redraw.clone(),
            _redraw: self.redraw.on_drop(),
        }
    }

    pub fn truncate_start(mut self, truncate_start: bool) -> Self {
        self.truncate_start = truncate_start;
        self
    }

    /// Label the picker without changing its prompt or result columns.
    pub fn with_title(mut self, title: impl Into<Line<'static>>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_preview(
        mut self,
        preview_fn: impl for<'a> Fn(&'a Editor, &'a T) -> Option<FileLocation<'a>> + 'static,
    ) -> Self {
        self.file_fn = Some(Box::new(preview_fn));
        // assumption: if we have a preview we are matching paths... If this is ever
        // not true this could be a separate builder function
        self.results
            .matcher
            .update_config(Config::DEFAULT.match_paths());
        self
    }

    pub fn with_custom_preview(mut self, preview: impl PickerPreview<T> + 'static) -> Self {
        self.custom_preview = Some(Box::new(preview));
        self
    }

    /// Defines how the contents of the picker are transformed into quicklist entries.
    pub fn with_quicklist(
        mut self,
        quicklist_fn: impl Fn(&Editor, &T) -> Option<QuicklistEntry> + 'static,
    ) -> Self {
        self.quicklist_fn = Some(Box::new(quicklist_fn));
        self
    }

    pub fn with_history_register(mut self, history_register: Option<char>) -> Self {
        self.prompt.with_history_register(history_register);
        self
    }

    pub fn with_initial_cursor(mut self, cursor: u32) -> Self {
        self.results.cursor = cursor;
        self
    }

    pub fn with_dynamic_query(
        mut self,
        callback: DynQueryCallback<T, D>,
        debounce_ms: Option<u64>,
        jobs: &crate::job::Jobs,
    ) -> Self {
        let handler = DynamicQueryHandler::new(
            callback,
            debounce_ms,
            jobs.sender(),
            self.results.version.clone(),
        )
        .spawn();
        let event = DynamicQueryChange {
            query: self.primary_query(),
            // Treat the initial query as a paste.
            is_paste: true,
        };
        event::send_blocking(&handler, event);
        self.dynamic_query_pending = true;
        self.dynamic_query_handler = Some(handler);
        self
    }

    pub fn with_default_action(mut self, action: Action) -> Self {
        self.default_action = action;
        self
    }

    /// Install the bindings and selected-item actions for a specific picker.
    pub(crate) fn with_item_actions(
        mut self,
        keymap: fn(&ComponentKeymaps) -> &Keymap<PickerAction>,
        callback: impl Fn(&mut Context, Option<&T>, PickerItemAction) -> PickerCallbackResult<T, D>
            + 'static,
    ) -> Self {
        self.keymap = keymap;
        self.item_action_fn = Some(Box::new(callback));
        self
    }

    #[cfg(feature = "git")]
    pub(crate) fn with_help(mut self, help: fn(&ComponentKeymaps) -> String) -> Self {
        self.help = Some(help);
        self
    }

    /// Move the cursor by a number of lines, either down (`Forward`) or up (`Backward`)
    pub fn move_by(&mut self, amount: u32, direction: Direction) {
        let len = self.results.matcher.snapshot().matched_item_count();

        if len == 0 {
            // No results, can't move.
            return;
        }

        match direction {
            Direction::Forward => {
                self.results.cursor = self.results.cursor.saturating_add(amount) % len;
            }
            Direction::Backward => {
                self.results.cursor = self
                    .results
                    .cursor
                    .saturating_add(len)
                    .saturating_sub(amount)
                    % len;
            }
        }
    }

    /// Move the cursor down by exactly one page. After the last page comes the first page.
    pub fn page_up(&mut self) {
        self.move_by(self.completion_height as u32, Direction::Backward);
    }

    /// Move the cursor up by exactly one page. After the first page comes the last page.
    pub fn page_down(&mut self) {
        self.move_by(self.completion_height as u32, Direction::Forward);
    }

    /// Move the cursor to the first entry
    pub fn to_start(&mut self) {
        self.results.cursor = 0;
    }

    /// Move the cursor to the last entry
    pub fn to_end(&mut self) {
        self.results.cursor = self
            .results
            .matcher
            .snapshot()
            .matched_item_count()
            .saturating_sub(1);
    }

    pub fn selection(&self) -> Option<&T> {
        self.results
            .matcher
            .snapshot()
            .get_matched_item(self.results.cursor)
            .map(|item| item.data)
    }

    fn apply_callback_result(
        &mut self,
        result: PickerCallbackResult<T, D>,
        editor: &Editor,
    ) -> PickerClose {
        match result {
            PickerCallbackResult::Close => PickerClose::Close(None),
            PickerCallbackResult::CloseWithCallback(callback) => PickerClose::Close(Some(callback)),
            PickerCallbackResult::KeepOpen => PickerClose::KeepOpen(None),
            #[cfg(feature = "git")]
            PickerCallbackResult::KeepOpenWithCallback(callback) => {
                PickerClose::KeepOpen(Some(callback))
            }
            PickerCallbackResult::Replace {
                options,
                editor_data,
            } => {
                self.replace_options(options, editor_data, editor);
                PickerClose::KeepOpen(None)
            }
            PickerCallbackResult::Refresh {
                options,
                editor_data,
            } => {
                self.refresh_options(options, Arc::new(editor_data));
                PickerClose::KeepOpen(None)
            }
        }
    }

    fn replace_options(&mut self, options: Vec<T>, editor_data: D, editor: &Editor) {
        self.results.cursor = 0;
        self.results.offset = 0;

        self.prompt.clear(editor);
        self.handle_prompt_change(false);
        self.reset_options(options, editor_data);
    }

    fn reset_options(&mut self, options: Vec<T>, editor_data: D) {
        if let Some(updates) = &mut self.file_updates {
            updates.stop();
        }
        self.results.discard_pending();
        self.reset_options_shared(options, Arc::new(editor_data));
    }

    fn reset_options_shared(&mut self, options: Vec<T>, editor_data: Arc<D>) {
        self.results.reset(options, editor_data, &self.columns);
        self.widths = visible_column_widths(&self.columns);
        self.preview.clear();
        if let Some(preview) = &mut self.custom_preview {
            preview.invalidate();
        }
    }

    /// Collects the picker's current matched items into quicklist entries.
    fn quicklist_entries(&self, editor: &Editor) -> Vec<QuicklistEntry> {
        let snapshot = self.results.matcher.snapshot();
        let mut entries = Vec::with_capacity(snapshot.matched_item_count() as usize);

        if let Some(quicklist_fn) = &self.quicklist_fn {
            for item in snapshot.matched_items(0..snapshot.matched_item_count()) {
                let Some(entry) = quicklist_fn(editor, item.data) else {
                    continue;
                };
                entries.push(entry);
            }
            return entries;
        }

        let Some(file_fn) = &self.file_fn else {
            return Vec::new();
        };

        for item in snapshot.matched_items(0..snapshot.matched_item_count()) {
            let Some((path_or_id, line_range)) = file_fn(editor, item.data) else {
                continue;
            };

            // TODO: quicklist currently captures the preview location only, which is
            // usually just a path plus an optional coarse line span. Extend this
            // handoff to preserve picker-specific jump metadata such as exact
            // columns, selections, or offset-encoding-aware LSP ranges.
            let target = match path_or_id {
                PathOrId::Path(path) => QuicklistTarget::Path(path.to_path_buf()),
                PathOrId::Id(id) => QuicklistTarget::Document(id),
            };
            let position = line_range.map_or(QuicklistPosition::None, |(start, end)| {
                QuicklistPosition::LineRange { start, end }
            });

            entries.push(QuicklistEntry { target, position });
        }

        entries
    }
    fn primary_query(&self) -> Arc<str> {
        self.query
            .get(&self.columns[self.primary_column].name)
            .cloned()
            .unwrap_or_else(|| "".into())
    }

    fn header_height(&self) -> u16 {
        if self.columns.len() > 1 {
            1
        } else {
            0
        }
    }

    pub fn toggle_preview(&mut self) {
        self.show_preview = !self.show_preview;
    }

    fn prompt_handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let search_active = !self.replacement_active();
        let result = self.active_prompt_mut().handle_event(event, cx);
        if search_active && matches!(result, EventResult::Consumed(_)) {
            self.handle_prompt_change(matches!(event, Event::Paste(_)));
        }
        EventResult::Consumed(None)
    }

    fn handle_prompt_change(&mut self, is_paste: bool) {
        // TODO: better track how the pattern has changed
        let line = self.prompt.line();
        let old_query = self.query.parse(line);
        if self.query == old_query {
            return;
        }
        // If the query has meaningfully changed, reset the cursor to the top of the results.
        self.results.cursor = 0;
        self.results.offset = 0;
        let primary_changed = &*self.primary_query()
            != old_query
                .get(&self.columns[self.primary_column].name)
                .map(|pattern| &**pattern)
                .unwrap_or_default();
        if primary_changed && self.dynamic_query_handler.is_some() {
            if self.results.pending_options.is_some()
                && let Some(updates) = &mut self.file_updates
            {
                updates.stop();
            }
            self.results.discard_pending();

            self.query_refresh = false;
            self.query_needs_refresh = false;
        } else if let Some(pending) = &mut self.results.pending_options {
            pending.preserve_selection = false;
        }
        // Have nucleo reparse each changed column.
        for (i, column) in self
            .columns
            .iter()
            .filter(|column| column.filter)
            .enumerate()
        {
            let pattern = self
                .query
                .get(&column.name)
                .map(|f| &**f)
                .unwrap_or_default();
            let old_pattern = old_query
                .get(&column.name)
                .map(|f| &**f)
                .unwrap_or_default();
            // Fastlane: most columns will remain unchanged after each edit.
            if pattern == old_pattern {
                continue;
            }
            let is_append = pattern.starts_with(old_pattern);
            self.results.reparse(i, pattern, is_append);
        }
        // If this is a dynamic picker, notify the query hook that the primary
        // query might have been updated.
        if let Some(handler) = &self.dynamic_query_handler {
            let previous = old_query
                .get(&self.columns[self.primary_column].name)
                .map(|pattern| &**pattern)
                .unwrap_or_default();
            if &*self.primary_query() != previous {
                self.dynamic_query_pending = true;
                if let Some(updates) = &mut self.editor_updates {
                    updates.task.cancel();
                }
                self.dynamic_query_request.cancel();
                self.results.invalidate();
            }
            let event = DynamicQueryChange {
                query: self.primary_query(),
                is_paste,
            };
            event::send_blocking(handler, event);
        }
    }

    /// Get (cached) preview for the currently selected item. If a document corresponding
    /// to the path is already open in the editor, it is used instead.
    fn get_preview<'picker, 'editor>(
        &'picker mut self,
        editor: &'editor Editor,
        jobs: &crate::job::Jobs,
        image_picker: Option<&ratatui_image::picker::Picker>,
        image_size: Size,
    ) -> Option<(Preview<'picker, 'editor>, Option<(usize, usize)>)> {
        let snapshot = self.results.matcher.snapshot();
        let location = snapshot
            .get_matched_item(self.results.cursor)
            .and_then(|current| (self.file_fn.as_ref()?)(editor, current.data));
        let Some(location) = location else {
            self.preview.cancel_loads(None);
            return None;
        };
        self.preview
            .get::<T, D>(editor, location, jobs, image_picker, image_size)
    }

    fn render_picker(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.publish_options();
        let status = self.results.matcher.tick(10);
        let snapshot = self.results.matcher.snapshot();
        if status.changed {
            self.results.cursor = self
                .results
                .cursor
                .min(snapshot.matched_item_count().saturating_sub(1))
        }

        let text_style = cx.editor.theme.get("ui.text");
        let selected = cx.editor.theme.get("ui.text.focus");
        let highlight_style = cx.editor.theme.get("special").add_modifier(Modifier::BOLD);

        // -- Render the frame:
        // clear area
        let background = cx.editor.theme.get("ui.background");
        surface.clear_with(area, background);

        let mut block = panel::bordered(&cx.editor.theme, cx.editor.config().border_style);
        if let Some(title) = &self.title {
            block = block.title(title.clone()).title_style(text_style);
        }
        let inner = block.inner(area);
        block.render(area, surface);
        if let Some(help) = self.help
            && area.height > 0
            && area.width > 4
        {
            surface.set_stringn(
                area.x + 2,
                area.bottom() - 1,
                help(&cx.config.current.component_keys),
                area.width.saturating_sub(4) as usize,
                cx.editor.theme.get("ui.text.inactive"),
            );
        }

        let input_areas = split_picker_input_area(inner, self.replacement_visible());
        let inner = input_areas.results;

        // -- Render the input bar:

        let count = format!(
            "{}{}{}/{}",
            self.case_sensitive.as_ref().map_or("", |toggle| {
                if toggle.enabled {
                    "case-sensitive "
                } else {
                    "smart-case "
                }
            }),
            if self.dynamic_query_pending
                || status.running
                || self.results.matcher.active_injectors() > 0
            {
                "(running) "
            } else {
                ""
            },
            snapshot.matched_item_count(),
            snapshot.item_count(),
        );

        let prompt_area = input_areas.search.clip_left(1);
        let [line_area, count_area, _right_padding] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(count.len() as u16),
            Constraint::Length(1),
        ])
        .areas(prompt_area);

        // render the prompt first since it will clear its background
        self.prompt.render(line_area, surface, cx);
        if let Some(area) = input_areas.replacement {
            self.replacement.as_mut().unwrap().prompt.render(
                area.clip_left(1).clip_right(1),
                surface,
                cx,
            );
        }

        Paragraph::new(count)
            .style(text_style)
            .render(count_area, surface);

        let sep_style = cx.editor.theme.get("ui.background.separator");
        panel::top_border(sep_style, cx.editor.config().border_style)
            .render(input_areas.search_separator, surface);
        if let Some(area) = input_areas.replacement_separator {
            panel::top_border(sep_style, cx.editor.config().border_style).render(area, surface);
        }

        let rows = inner.height.saturating_sub(self.header_height()) as u32;
        let rows = rows.max(1);
        self.results.offset = self.results.offset.min(self.results.cursor);
        if self.results.cursor >= self.results.offset.saturating_add(rows) {
            self.results.offset = self.results.cursor + 1 - rows;
        }
        let offset = self.results.offset;
        let cursor = self.results.cursor.saturating_sub(offset);
        let end = offset
            .saturating_add(rows)
            .min(snapshot.matched_item_count());
        let mut indices = Vec::new();
        self.highlight_matcher.set_kind(if self.file_fn.is_some() {
            fuzzy::MatchKind::Path
        } else {
            fuzzy::MatchKind::Text
        });
        let matcher = &mut self.highlight_matcher;

        let options = snapshot.matched_items(offset..end).map(|item| {
            let mut widths = self.widths.iter_mut();
            let mut matcher_index = 0;

            Row::new(self.columns.iter().filter_map(|column| {
                if column.hidden {
                    return None;
                }

                let Some(Constraint::Length(max_width)) = widths.next() else {
                    unreachable!();
                };
                let mut cell = column.format(item.data, &self.results.editor_data);
                let mut width = if column.filter {
                    matcher.indices(
                        snapshot.pattern().column_pattern(matcher_index),
                        item.matcher_columns[matcher_index].slice(..),
                        &mut indices,
                    );
                    indices.sort_unstable();
                    indices.dedup();
                    let mut indices = indices.drain(..);
                    let mut next_highlight_idx = indices.next().unwrap_or(u32::MAX);
                    let mut span_list = Vec::new();
                    let mut current_span = String::new();
                    let mut current_style = tui::style::Style::default();
                    let mut grapheme_idx = 0u32;
                    let mut width = 0;

                    let spans: &[Span] = cell
                        .content
                        .lines
                        .first()
                        .map_or(&[], |line| line.spans.as_slice());
                    for span in spans {
                        // this looks like a bug on first glance, we are iterating
                        // graphemes but treating them as char indices. The reason that
                        // this is correct is that nucleo will only ever consider the first char
                        // of a grapheme (and discard the rest of the grapheme) so the indices
                        // returned by nucleo are essentially grapheme indecies
                        for grapheme in span.content.graphemes(true) {
                            let style = if grapheme_idx == next_highlight_idx {
                                next_highlight_idx = indices.next().unwrap_or(u32::MAX);
                                span.style.patch(tui::style::Style::from(highlight_style))
                            } else {
                                span.style
                            };
                            if style != current_style {
                                if !current_span.is_empty() {
                                    span_list.push(Span::styled(current_span, current_style))
                                }
                                current_span = String::new();
                                current_style = style;
                            }
                            current_span.push_str(grapheme);
                            grapheme_idx += 1;
                        }
                        width += span.width();
                    }

                    span_list.push(Span::styled(current_span, current_style));
                    cell = Cell::from(Line::from(span_list));
                    matcher_index += 1;
                    width
                } else {
                    cell.content
                        .lines
                        .first()
                        .map(|line| line.width())
                        .unwrap_or_default()
                };

                if let Some(prefix) = (column.prefix)(item.data, &self.results.editor_data) {
                    width += prefix.width();
                    cell = cell.with_prefix(prefix);
                }

                if width as u16 > *max_width {
                    *max_width = width as u16;
                }

                Some(cell)
            }))
        });

        let mut table = Table::new(options)
            .style(text_style)
            .highlight_style(selected)
            .highlight_symbol(" > ")
            .column_spacing(1)
            .widths(&self.widths);

        // -- Header
        if self.columns.len() > 1 {
            let active_column = self.query.active_column(self.prompt.position());
            let header_style = cx.editor.theme.get("ui.picker.header");
            let header_column_style = cx.editor.theme.get("ui.picker.header.column");

            table = table.header(
                Row::new(self.columns.iter().filter_map(|column| {
                    if column.hidden {
                        return None;
                    }

                    let style = if active_column.is_some_and(|name| Arc::ptr_eq(name, &column.name))
                    {
                        cx.editor.theme.get("ui.picker.header.column.active")
                    } else {
                        header_column_style
                    };

                    Some(Cell::from(Span::styled(Cow::from(&*column.name), style)))
                }))
                .style(header_style),
            );
        }

        use tui::widgets::TableState;

        let mut table_state = TableState::default().with_selected(Some(cursor as usize));

        table.render_table(inner, surface, &mut table_state, self.truncate_start);
    }

    fn render_preview(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let background = cx.editor.theme.get("ui.background");
        surface.clear_with(area, background);

        let block = panel::horizontally_padded(&cx.editor.theme, cx.editor.config().border_style);
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.is_empty() {
            self.preview.cancel_loads(None);
            if let Some(preview) = &mut self.custom_preview {
                preview.cancel();
            }
            return;
        }
        if let Some(preview) = &mut self.custom_preview {
            let item = self
                .results
                .matcher
                .snapshot()
                .get_matched_item(self.results.cursor)
                .map(|item| item.data);
            preview.render(item, inner, surface, cx);
            return;
        }
        let image_size = Size::new(inner.width, inner.height.saturating_sub(2).max(1));
        if let Some((preview, range)) =
            self.get_preview(cx.editor, cx.jobs, cx.image_picker, image_size)
        {
            render_preview_content(preview, range, area, inner, surface, cx);
        }
    }

    fn execute_actions(&mut self, actions: &[PickerAction], ctx: &mut Context) -> EventResult {
        let close_fn = |picker: &mut Self, after: Option<compositor::Callback>| {
            picker.query_needs_refresh =
                picker.dynamic_query_handler.is_some() && picker.file_updates.is_none();
            picker.results.discard_pending();
            picker.results.spare_matcher = None;

            picker.dynamic_query_pending = false;
            picker.query_refresh = false;
            picker.dynamic_query_request.cancel();
            if let Some(updates) = &mut picker.editor_updates {
                updates.task.cancel();
                updates.resumed = true;
            }
            picker.poll = None;
            if let Some(updates) = &mut picker.file_updates {
                updates.stop();
            }
            picker.preview.cancel_loads(None);
            if let Some(preview) = &mut picker.custom_preview {
                preview.cancel();
            }
            // if the picker is very large don't store it as last_picker to avoid
            // excessive memory consumption
            let callback: compositor::Callback =
                if picker.results.matcher.snapshot().item_count() > 1_000_000 {
                    Box::new(|compositor: &mut Compositor, _ctx| {
                        // remove the layer
                        compositor.pop();
                    })
                } else {
                    // stop streaming in new items in the background, really we should
                    // be restarting the stream somehow once the picker gets
                    // reopened instead (like for an FS crawl) that would also remove the
                    // need for the special case above but that is pretty tricky
                    picker
                        .results
                        .version
                        .fetch_add(1, atomic::Ordering::Relaxed);
                    Box::new(|compositor: &mut Compositor, _ctx| {
                        // remove the layer
                        compositor.last_picker = compositor.pop();
                    })
                };
            EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                callback(compositor, cx);
                if let Some(after) = after {
                    after(compositor, cx);
                }
            })))
        };

        // A restored picker can receive input before its first redraw.
        self.refresh_from_editor(ctx.editor);
        for &action in actions {
            // Keep navigation available while replacements load or a resumed
            // picker waits for its old matcher before checking the current source.
            if (self.results.is_refreshing()
                || self
                    .editor_updates
                    .as_ref()
                    .is_some_and(|updates| updates.resumed))
                && matches!(
                    action,
                    PickerAction::Accept
                        | PickerAction::AcceptKeepOpen
                        | PickerAction::AcceptHorizontalSplit
                        | PickerAction::AcceptVerticalSplit
                        | PickerAction::Item(_)
                        | PickerAction::PopulateQuicklist
                )
            {
                return EventResult::Consumed(None);
            }
            if matches!(
                action,
                PickerAction::Accept
                    | PickerAction::AcceptKeepOpen
                    | PickerAction::AcceptHorizontalSplit
                    | PickerAction::AcceptVerticalSplit
            ) {
                let opens_preview = self
                    .custom_preview
                    .as_ref()
                    .zip(self.selection())
                    .is_some_and(|(preview, item)| preview.open_on_accept(item));
                if opens_preview {
                    let item = self
                        .results
                        .matcher
                        .snapshot()
                        .get_matched_item(self.results.cursor)
                        .map(|item| item.data);
                    let open = match action {
                        PickerAction::AcceptHorizontalSplit => Action::HorizontalSplit,
                        PickerAction::AcceptVerticalSplit => Action::VerticalSplit,
                        _ => self.default_action,
                    };
                    let result = self.custom_preview.as_mut().unwrap().action(
                        item,
                        PreviewAction::Open(open),
                        ctx,
                    );
                    if self.custom_preview.as_ref().unwrap().close_on_open()
                        && action != PickerAction::AcceptKeepOpen
                        && let EventResult::Consumed(callback) = result
                    {
                        return close_fn(self, callback);
                    }
                    return result;
                }
            }
            match action {
                PickerAction::MoveUp => {
                    self.move_by(1, Direction::Backward);
                }
                PickerAction::MoveDown => {
                    self.move_by(1, Direction::Forward);
                }
                PickerAction::PageDown => {
                    self.page_down();
                }
                PickerAction::PageUp => {
                    self.page_up();
                }
                PickerAction::MoveStart => {
                    self.to_start();
                }
                PickerAction::MoveEnd => {
                    self.to_end();
                }
                PickerAction::Cancel => return close_fn(self, None),
                PickerAction::PopulateQuicklist => {
                    // Pickers can provide explicit quicklist entries when they have
                    // more precise jump data. Everything else falls back to preview
                    // locations, which are still usually just path + line span.
                    let entries = self.quicklist_entries(ctx.editor);
                    let count = entries.len();
                    ctx.editor.replace_quicklist(entries);
                    if count == 0 {
                        ctx.editor
                            .set_status("No quicklist entries available for this picker");
                    } else {
                        ctx.editor
                            .set_status(format!("Quicklist populated with {count} entries"));
                    }
                }
                PickerAction::AcceptKeepOpen => {
                    if let Some(option) = self.selection() {
                        let result = (self.callback_fn)(ctx, option, self.default_action);
                        if let PickerClose::Close(Some(callback))
                        | PickerClose::KeepOpen(Some(callback)) =
                            self.apply_callback_result(result, ctx.editor)
                        {
                            return EventResult::Consumed(Some(callback));
                        }
                    }
                }
                PickerAction::ToggleReplacement => self.toggle_replacement_prompt(),
                PickerAction::ToggleCaseSensitive => self.toggle_case_sensitive(ctx.editor),
                PickerAction::ReplaceAll => {
                    if self.replacement_visible() {
                        self.replace_matches(ctx, true);
                        return EventResult::Consumed(None);
                    }
                }
                PickerAction::Accept if self.replacement_visible() => {
                    self.replace_matches(ctx, false);
                    return EventResult::Consumed(None);
                }
                PickerAction::Accept => {
                    // If the prompt has a history completion and is empty, use enter to accept
                    // that completion
                    if let Some(completion) = self
                        .prompt
                        .first_history_completion(ctx.editor)
                        .filter(|_| self.prompt.line().is_empty())
                    {
                        // The percent character is used by the query language and needs to be
                        // escaped with a backslash.
                        let completion = if completion.contains('%') {
                            completion.replace('%', "\\%")
                        } else {
                            completion.into_owned()
                        };
                        self.prompt.set_line(completion, ctx.editor);

                        // Inserting from the history register is a paste.
                        self.handle_prompt_change(true);
                    } else {
                        let callback_result = self
                            .selection()
                            .map_or(PickerCallbackResult::Close, |option| {
                                (self.callback_fn)(ctx, option, self.default_action)
                            });
                        if let Some(history_register) = self.prompt.history_register()
                            && let Err(err) = ctx
                                .editor
                                .registers
                                .push(history_register, self.primary_query().to_string())
                        {
                            ctx.editor.set_error(|| err.to_string());
                        }
                        match self.apply_callback_result(callback_result, ctx.editor) {
                            PickerClose::Close(callback) => return close_fn(self, callback),
                            PickerClose::KeepOpen(Some(callback)) => {
                                return EventResult::Consumed(Some(callback))
                            }
                            PickerClose::KeepOpen(None) => {}
                        }
                    }
                }
                PickerAction::AcceptHorizontalSplit | PickerAction::AcceptVerticalSplit => {
                    let action = match action {
                        PickerAction::AcceptHorizontalSplit => Action::HorizontalSplit,
                        _ => Action::VerticalSplit,
                    };
                    let callback_result = self
                        .selection()
                        .map_or(PickerCallbackResult::Close, |option| {
                            (self.callback_fn)(ctx, option, action)
                        });
                    match self.apply_callback_result(callback_result, ctx.editor) {
                        PickerClose::Close(callback) => return close_fn(self, callback),
                        PickerClose::KeepOpen(Some(callback)) => {
                            return EventResult::Consumed(Some(callback))
                        }
                        PickerClose::KeepOpen(None) => {}
                    }
                }
                PickerAction::PreviewScrollUp
                | PickerAction::PreviewScrollDown
                | PickerAction::PreviewPrevious
                | PickerAction::PreviewNext
                | PickerAction::OpenPreview => {
                    if let Some(preview) = &mut self.custom_preview {
                        let action = match action {
                            PickerAction::PreviewScrollUp => PreviewAction::ScrollUp,
                            PickerAction::PreviewScrollDown => PreviewAction::ScrollDown,
                            PickerAction::PreviewPrevious => PreviewAction::Previous,
                            PickerAction::PreviewNext => PreviewAction::Next,
                            _ => PreviewAction::Open(self.default_action),
                        };
                        let item = self
                            .results
                            .matcher
                            .snapshot()
                            .get_matched_item(self.results.cursor)
                            .map(|item| item.data);
                        let result = preview.action(item, action, ctx);
                        let close =
                            preview.close_on_open() && matches!(action, PreviewAction::Open(_));
                        if close && let EventResult::Consumed(callback) = result {
                            return close_fn(self, callback);
                        }
                        return result;
                    }
                }
                PickerAction::TogglePreview => {
                    self.toggle_preview();
                }
                PickerAction::Item(action) => {
                    if let Some(handler) = &self.item_action_fn {
                        let result = handler(ctx, self.selection(), action);
                        match self.apply_callback_result(result, ctx.editor) {
                            PickerClose::Close(callback) => return close_fn(self, callback),
                            PickerClose::KeepOpen(callback) => {
                                return EventResult::Consumed(callback)
                            }
                        }
                    }
                    // The action may replace the asynchronous matcher contents.
                    // Further actions must wait for the refreshed selection.
                    return EventResult::Consumed(None);
                }
            }
        }

        EventResult::Consumed(None)
    }
}

impl<I: 'static + Send + Sync, D: 'static + Send + Sync> Component for Picker<I, D> {
    #[cfg(feature = "integration")]
    fn has_pending_render(&mut self) -> bool {
        let pending = self
            .results
            .pending_options
            .as_mut()
            .is_some_and(|pending| {
                let status = pending.matcher.tick(0);
                status.running || status.changed || pending.matcher.active_injectors() == 0
            });
        let status = self.results.matcher.tick(0);
        let pending = pending || status.running || status.changed;
        if pending {
            self.redraw.request();
        }
        pending
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // +---------+ +---------+
        // |prompt   | |preview  |
        // +---------+ |         |
        // |picker   | |         |
        // |         | |         |
        // +---------+ +---------+

        let render_preview = self.show_preview
            && (self.file_fn.is_some() || self.custom_preview.is_some())
            && area.width > MIN_AREA_WIDTH_FOR_PREVIEW;

        let poll = &cx.editor.config().files.reload.poll;
        if (render_preview || self.file_updates.is_some() || self.query_updates.is_some())
            && poll.enable
        {
            let interval = std::time::Duration::from_millis(poll.interval.max(100));
            if self
                .poll
                .as_ref()
                .is_none_or(|poll| poll.interval != interval)
            {
                let redraw = self.redraw.clone();
                let task = tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(interval).await;
                        redraw.request();
                    }
                });
                self.poll = Some(PollTask { task, interval });
            }
        } else {
            self.poll = None;
        }

        self.refresh_files(cx.editor);
        self.refresh_query_if_changed(cx.editor);
        self.refresh_from_editor(cx.editor);

        let (picker_area, preview_area) = split_picker_area(area, render_preview);
        self.render_picker(picker_area, surface, cx);

        if let Some(preview_area) = preview_area {
            self.render_preview(preview_area, surface, cx);
        } else {
            self.preview.cancel_loads(None);
            if let Some(preview) = &mut self.custom_preview {
                preview.cancel();
            }
        }
    }

    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending() || self.active_prompt().has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut Context) -> EventResult {
        // TODO: keybinds for scrolling preview

        let key_event = match event {
            Event::Key(event) => *event,
            Event::Paste(..) => {
                self.keys.clear();
                return self.prompt_handle_event(event, ctx);
            }
            Event::Resize(..) => return EventResult::Consumed(None),
            // Picker is a modal and should consume mouse events so clicks don't fall
            // through to the editor underneath
            Event::Mouse(_) => return EventResult::Consumed(None),
            _ => return EventResult::Ignored(None),
        };

        // Once the embedded prompt starts a key sequence or register selection,
        // it owns subsequent keys, including keys normally handled by the picker.
        if self.active_prompt().has_pending_keys() {
            return self.prompt_handle_event(event, ctx);
        }
        let actions = match self
            .keys
            .lookup((self.keymap)(&ctx.config.current.component_keys), key_event)
        {
            Lookup::Actions(actions) => actions,
            Lookup::Pending => return EventResult::Consumed(None),
            Lookup::Unbound => return self.prompt_handle_event(event, ctx),
        };

        self.execute_actions(actions, ctx)
    }

    fn owns_cursor(&self) -> bool {
        true
    }

    fn cursor(&self, area: Rect, editor: &Editor) -> Cursor {
        let render_preview = self.show_preview
            && (self.file_fn.is_some() || self.custom_preview.is_some())
            && area.width > MIN_AREA_WIDTH_FOR_PREVIEW;
        let (picker_area, _) = split_picker_area(area, render_preview);
        let inner = panel::bordered(&editor.theme, editor.config().border_style).inner(picker_area);
        let areas = split_picker_input_area(inner, self.replacement_visible());
        let area = if self.replacement_active() {
            areas.replacement.unwrap().clip_right(1)
        } else {
            areas.search
        };
        self.active_prompt().cursor(area.clip_left(1), editor)
    }

    fn required_size(&mut self, (width, height): (u16, u16)) -> Option<(u16, u16)> {
        self.completion_height = height.saturating_sub(
            4 + self.header_height() + if self.replacement_visible() { 2 } else { 0 },
        );
        Some((width, height))
    }

    fn id(&self) -> Option<&'static str> {
        Some(ID)
    }
}

enum PickerClose {
    KeepOpen(Option<compositor::Callback>),
    Close(Option<compositor::Callback>),
}

pub(crate) enum PickerCallbackResult<T, D> {
    Close,
    CloseWithCallback(compositor::Callback),
    KeepOpen,
    #[cfg(feature = "git")]
    KeepOpenWithCallback(compositor::Callback),
    Replace {
        options: Vec<T>,
        editor_data: D,
    },
    /// Preserve the query and restore the nearest row after matching finishes.
    Refresh {
        options: Vec<T>,
        editor_data: D,
    },
}

type PickerCallback<T, D> = Box<dyn Fn(&mut Context, &T, Action) -> PickerCallbackResult<T, D>>;
type PickerItemCallback<T, D> =
    Box<dyn Fn(&mut Context, Option<&T>, PickerItemAction) -> PickerCallbackResult<T, D>>;

#[cfg(test)]
mod tests {
    use super::{split_picker_area, visible_column_widths, Column};
    use crate::ui::image::image_preview_layout;
    use tui::layout::{Constraint, Size};
    use view::graphics::Rect;

    #[test]
    fn preview_uses_the_right_half_of_the_picker() {
        let area = Rect::new(3, 5, 81, 24);
        let (picker, preview) = split_picker_area(area, true);

        assert_eq!(Rect::new(3, 5, 40, 24), picker);
        assert_eq!(Some(Rect::new(43, 5, 41, 24)), preview);
        assert_eq!((area, None), split_picker_area(area, false));
    }

    #[test]
    fn image_and_caption_are_centered_as_a_group() {
        let area = Rect::new(10, 5, 60, 30);
        let (image, caption) = image_preview_layout(area, Size::new(20, 10));
        assert_eq!(image, Rect::new(30, 14, 20, 10));
        assert_eq!(caption, Rect::new(10, 25, 60, 1));
        assert_eq!(image.y - area.y, area.bottom() - caption.bottom());

        let (image, caption) = image_preview_layout(area, Size::new(8, 28));
        assert_eq!(image, Rect::new(36, 5, 8, 28));
        assert_eq!(caption, Rect::new(10, 34, 60, 1));
    }

    #[test]
    fn image_layout_stays_inside_tiny_previews() {
        for width in 0..3 {
            for height in 0..3 {
                let area = Rect::new(10, 5, width, height);
                let (image, caption) = image_preview_layout(area, Size::new(20, 10));
                assert_eq!(image.intersection(area), image);
                assert_eq!(caption.intersection(area), caption);
            }
        }
    }

    #[test]
    fn hidden_columns_do_not_consume_rendered_width() {
        let columns: [Column<(), ()>; 2] = [
            Column::new("path", |_, _| "".into()),
            Column::hidden("contents"),
        ];

        assert_eq!(visible_column_widths(&columns), [Constraint::Length(4)]);
    }

    #[cfg(feature = "integration")]
    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_preserves_cursor_and_blocks_row_actions_until_matching_finishes(
    ) -> anyhow::Result<()> {
        use super::*;
        use crate::{application::Application, config::Config as AppConfig, job::Jobs};
        use std::{cell::RefCell, rc::Rc, time::Duration};

        let mut config = AppConfig::default();
        config.editor.files.watch.enable = false;
        config.editor.files.watch.watch_vcs = false;
        let syntax = language::syntax::Loader::new(
            language_config::Configuration {
                language: vec![],
                language_server: Default::default(),
            },
            Default::default(),
        )?;
        let mut app = Application::new(
            Default::default(),
            config.clone(),
            syntax,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        )?;
        let mut jobs = Jobs::new();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            jobs: &mut jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: false,
        };
        let accepted = Rc::new(RefCell::new(Vec::new()));
        let recorded = accepted.clone();
        let removed = Rc::new(RefCell::new(Vec::new()));
        let removed_items = removed.clone();
        let mut picker = Picker::new_with_callback_result(
            cx.editor.redraw().clone(),
            [Column::new("name", |item: &u8, _: &()| {
                item.to_string().into()
            })],
            0,
            [1, 2, 3],
            (),
            move |_, item, _| {
                recorded.borrow_mut().push(*item);
                PickerCallbackResult::Refresh {
                    options: vec![1, 2],
                    editor_data: (),
                }
            },
        )
        .with_initial_cursor(2)
        .with_item_actions(
            |maps| &maps.quicklist_picker,
            move |_, item, action| {
                assert_eq!(action, PickerItemAction::RemoveQuicklistEntry);
                removed_items
                    .borrow_mut()
                    .push(*item.expect("selected quicklist row"));
                PickerCallbackResult::KeepOpen
            },
        )
        .with_quicklist(|_, item| {
            Some(QuicklistEntry {
                target: QuicklistTarget::Path(std::path::PathBuf::from(format!("{item}.txt"))),
                position: QuicklistPosition::None,
            })
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while picker.results.matcher.tick(10).running {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let accept = Event::Key("ret".parse().unwrap());
        assert!(matches!(
            picker.handle_event(&accept, &mut cx),
            EventResult::Consumed(None)
        ));
        // Keep the replacement unpublished regardless of matcher scheduling.
        let pending_injector = picker
            .results
            .pending_options
            .as_ref()
            .unwrap()
            .matcher
            .injector();
        let area = Rect::new(0, 0, 80, 24);
        picker.render(area, &mut Surface::empty(area), &mut cx);
        let remove = Event::Key("C-x".parse().unwrap());
        let populate = Event::Key("C-q".parse().unwrap());
        for event in [&accept, &remove, &populate] {
            assert!(matches!(
                picker.handle_event(event, &mut cx),
                EventResult::Consumed(None)
            ));
        }
        assert_eq!(*accepted.borrow(), [3]);
        assert!(removed.borrow().is_empty());
        assert!(cx.editor.quicklist.entries().is_empty());
        assert_eq!(picker.results.cursor, 2);
        drop(pending_injector);
        tokio::time::timeout(Duration::from_secs(5), async {
            while picker.results.is_refreshing() {
                picker.render(area, &mut Surface::empty(area), &mut cx);
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(picker.results.cursor, 1);
        assert!(matches!(
            picker.handle_event(&remove, &mut cx),
            EventResult::Consumed(None)
        ));
        assert_eq!(*removed.borrow(), [2]);
        assert!(matches!(
            picker.handle_event(&populate, &mut cx),
            EventResult::Consumed(None)
        ));
        assert_eq!(cx.editor.quicklist.entries().len(), 2);
        assert!(matches!(
            picker.handle_event(&accept, &mut cx),
            EventResult::Consumed(None)
        ));
        assert_eq!(*accepted.borrow(), [3, 2]);
        assert!(matches!(
            picker.handle_event(&Event::Key("esc".parse().unwrap()), &mut cx),
            EventResult::Consumed(Some(_))
        ));
        assert!(app.close().await.is_empty());
        Ok(())
    }

    #[cfg(feature = "integration")]
    #[tokio::test(flavor = "multi_thread")]
    async fn resumed_picker_blocks_stale_row_actions_while_matcher_is_busy() -> anyhow::Result<()> {
        use super::*;
        use crate::{application::Application, config::Config as AppConfig, job::Jobs};
        use std::{
            cell::RefCell,
            rc::Rc,
            sync::{atomic::AtomicBool, mpsc, Mutex},
            time::Duration,
        };

        let mut config = AppConfig::default();
        config.editor.files.watch.enable = false;
        config.editor.files.watch.watch_vcs = false;
        let syntax = language::syntax::Loader::new(
            language_config::Configuration {
                language: vec![],
                language_server: Default::default(),
            },
            Default::default(),
        )?;
        let mut app = Application::new(
            Default::default(),
            config.clone(),
            syntax,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        )?;
        let mut jobs = Jobs::new();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            jobs: &mut jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: false,
        };

        let block_next = Arc::new(AtomicBool::new(false));
        let block_worker = block_next.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let matcher = Nucleo::new(
            Config::DEFAULT,
            Arc::new(move || {
                if block_worker.swap(false, atomic::Ordering::SeqCst) {
                    started_tx.send(()).unwrap();
                    // Hold the worker lock until the test releases this match.
                    let _ = release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5));
                }
            }),
            Some(1),
            1,
        );
        let acted_on = Rc::new(RefCell::new(Vec::new()));
        let accepted = acted_on.clone();
        let removed = acted_on.clone();
        let mut picker = Picker::with(
            cx.editor.redraw().clone(),
            matcher,
            Arc::new([Column::new("name", |item: &u8, _: &()| {
                item.to_string().into()
            })]),
            0,
            Arc::new(()),
            Arc::new(AtomicUsize::new(0)),
            move |_, item, _| {
                accepted.borrow_mut().push(*item);
                PickerCallbackResult::KeepOpen
            },
        )
        .with_editor_updates(|cx| cx.resumed.then(|| vec![4]))
        .with_item_actions(
            |maps| &maps.quicklist_picker,
            move |_, item, _| {
                removed
                    .borrow_mut()
                    .push(*item.expect("selected quicklist row"));
                PickerCallbackResult::KeepOpen
            },
        )
        .with_quicklist(|_, item| {
            Some(QuicklistEntry {
                target: QuicklistTarget::Path(format!("{item}.txt").into()),
                position: QuicklistPosition::None,
            })
        });
        // Release the worker before dropping the picker, including on assertion failures.
        let release_worker = release_tx;
        picker.reset_options(vec![1, 2], ());
        tokio::time::timeout(Duration::from_secs(5), async {
            while picker.results.matcher.tick(10).running {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(matches!(
            picker.handle_event(&Event::Key("esc".parse().unwrap()), &mut cx),
            EventResult::Consumed(Some(_))
        ));
        let expected = vec![QuicklistEntry {
            target: QuicklistTarget::Path("4.txt".into()),
            position: QuicklistPosition::None,
        }];
        cx.editor.replace_quicklist(expected.clone());

        // Start another match while keeping the cached snapshot visible.
        inject_nucleo_item(&picker.results.matcher.injector(), &picker.columns, 3, &());
        block_next.store(true, atomic::Ordering::SeqCst);
        picker.results.matcher.tick(0);
        started_rx.recv_timeout(Duration::from_secs(5))?;
        for key in ["C-q", "ret", "C-x"] {
            assert!(matches!(
                picker.handle_event(&Event::Key(key.parse().unwrap()), &mut cx),
                EventResult::Consumed(None)
            ));
        }
        assert_eq!(cx.editor.quicklist.entries(), expected.as_slice());
        assert!(acted_on.borrow().is_empty());
        picker.handle_event(&Event::Key("down".parse().unwrap()), &mut cx);
        assert_eq!(picker.results.cursor, 1);
        drop(release_worker);

        let area = Rect::new(0, 0, 80, 24);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                picker.render(area, &mut Surface::empty(area), &mut cx);
                if !picker.results.is_refreshing()
                    && !picker.editor_updates.as_ref().unwrap().resumed
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        for key in ["C-q", "ret", "C-x"] {
            picker.handle_event(&Event::Key(key.parse().unwrap()), &mut cx);
        }
        assert_eq!(cx.editor.quicklist.entries(), expected.as_slice());
        assert_eq!(*acted_on.borrow(), [4, 4]);
        assert!(app.close().await.is_empty());
        Ok(())
    }
}
