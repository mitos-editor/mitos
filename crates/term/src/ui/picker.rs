use crate::compositor::DrawContext;
use crate::keymap::component::{ComponentKeymaps, PickerAction, PickerItemAction};
use ui_core::keymap::{Keymap, KeymapState, Lookup};
mod file_updates;
mod handlers;
mod help;
#[cfg(test)]
mod matching_tests;
#[cfg(any(feature = "git", test))]
pub(crate) mod paged;
#[cfg(test)]
mod paging_tests;
mod preview;
mod query;
mod query_options;
mod replacement;
mod results;
pub(crate) mod scroll;
#[cfg(test)]
mod scroll_tests;
mod source;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
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
use tui::{
    buffer::Buffer as Surface,
    layout::{Constraint, Layout, Size},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, StatefulWidget, Table},
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

use self::handlers::{DynamicQuery, DynamicQueryChange, DynamicQueryHandler, QueryKind};
use self::preview::{render_preview_content, FilePreview, FileScroll};
use self::replacement::{split_picker_input_area, CaseSensitiveToggle, Replacement};

// These types remain available through the picker facade.
pub use self::preview::{
    CachedPreview, FileLocation, ImagePreview, PathOrId, Preview, MAX_FILE_SIZE_FOR_PREVIEW,
};
pub(crate) use replacement::{ReplacementStep, ReplacementTask};
#[cfg(any(feature = "git", test))]
pub(crate) use updates::PickerSource;
#[cfg(feature = "git")]
pub(crate) use updates::Update;
pub(crate) use updates::UpdateContext;

type FileCallback<T> = Box<dyn for<'a> Fn(&'a Editor, &'a T) -> Option<FileLocation<'a>>>;
type QuicklistCallback<T> = Box<dyn Fn(&Editor, &T) -> Option<QuicklistEntry>>;
type SelectedPreview<'picker, 'editor> = (
    Preview<'picker, 'editor>,
    Option<(usize, usize)>,
    &'picker mut FileScroll,
);

/// A native preview surface independent of file loading and repository providers.
/// Implementations own their background work and cancel it when hidden or dropped.
pub trait PickerPreview<T> {
    fn update(&mut self, _item: Option<&T>, _area: Rect, _cx: &mut Context) {}
    fn render(&mut self, item: Option<&T>, area: Rect, surface: &mut Surface, cx: &mut DrawContext);
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
    source: source::Progress,
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
            source: self.source.clone(),
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

    /// Producers explicitly report exhaustion; dropping a handle may mean cancellation.
    pub(crate) fn complete(&self) {
        self.report(source::State::Complete);
    }
    pub(crate) fn fail(&self) {
        self.report(source::State::Failed);
    }
    pub(crate) fn pause(&self) {
        self.report(source::State::Paused);
    }
    fn report(&self, state: source::State) {
        if self
            .source
            .report(state, self.version, &self.picker_version)
        {
            self.redraw.request();
        }
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
    help: Option<fn(&ComponentKeymaps) -> Vec<String>>,
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
    preview_area: Option<Rect>,
    /// Constraints for tabular formatting
    widths: Vec<Constraint>,

    callback_fn: PickerCallback<T, D>,
    /// Embedded pickers return focus to their parent instead of popping a layer.
    on_close: Option<fn(&mut Compositor, &mut Context)>,
    default_action: Action,

    pub truncate_start: bool,
    preview: FilePreview,
    preview_scroll: FileScroll,
    custom_preview: Option<Box<dyn PickerPreview<T>>>,
    /// Given an item in the picker, return the file path and line number to display.
    file_fn: Option<FileCallback<T>>,
    /// Given an item in the picker, return an explicit quicklist entry.
    quicklist_fn: Option<QuicklistCallback<T>>,
    replacement: Option<Replacement<T, D>>,
    case_sensitive: Option<CaseSensitiveToggle<D>>,
    dynamic_query: Option<DynamicQuery>,
    poll: Option<PollTask>,
    matching: bool,
    file_updates: Option<file_updates::FileUpdates<T, D>>,
    editor_updates: Option<updates::EditorUpdates<T>>,
    query_updates: Option<updates::QueryUpdates<D>>,
    query_options: Option<QueryOptions<T, D>>,
    async_query_options: Option<query_options::AsyncQueryOptions<T, D>>,
    pending_query: Option<query_options::PendingQuery<T, D>>,
    query_task: event::TaskController,
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
            preview_area: None,
            callback_fn: Box::new(move |cx, item, _, action| callback_fn(cx, item, action)),
            on_close: None,
            default_action: Action::Replace,
            completion_height: 0,
            widths,
            preview: FilePreview::default(),
            preview_scroll: FileScroll::default(),
            custom_preview: None,
            file_fn: None,
            quicklist_fn: None,
            replacement: None,
            case_sensitive: None,
            dynamic_query: None,
            poll: None,
            file_updates: None,
            editor_updates: None,
            query_updates: None,
            query_options: None,
            async_query_options: None,
            pending_query: None,
            query_task: event::TaskController::new(),
            query_needs_refresh: false,
            title: None,
            matching: true,
        }
    }

    pub fn injector(&self) -> Injector<T, D> {
        self.results.source.set(source::State::Loading);
        Injector {
            source: self.results.source.clone(),
            dst: self.results.matcher.injector(),
            columns: self.columns.clone(),
            editor_data: self.results.editor_data.clone(),
            version: self.results.version.load(atomic::Ordering::Relaxed),
            picker_version: self.results.version.clone(),
            redraw: self.redraw.clone(),
            _redraw: self.redraw.on_drop(),
        }
    }

    pub(super) fn editor_data(&self) -> &D {
        &self.results.editor_data
    }

    /// Let a hierarchical source retain context rows that do not themselves match.
    pub(super) fn with_query_options(
        mut self,
        callback: impl Fn(&Editor, &str, &D, Option<&T>) -> anyhow::Result<(Vec<T>, D, usize)> + 'static,
    ) -> Self {
        self.query_options = Some(Box::new(callback));
        self
    }

    pub(super) fn with_data_callback(
        mut self,
        callback: impl Fn(&mut Context, &T, &D, Action) -> PickerCallbackResult<T, D> + 'static,
    ) -> Self {
        self.callback_fn = Box::new(callback);
        self
    }

    pub(crate) fn embedded(mut self, on_close: fn(&mut Compositor, &mut Context)) -> Self {
        self.on_close = Some(on_close);
        self.show_preview = false;
        self
    }

    /// Stop background source activity while retaining published rows and selection.
    /// Restoring the picker resumes its sources through their existing update path.
    pub(crate) fn suspend(&mut self) {
        self.query_needs_refresh |= self.pending_query.is_some()
            || (self.async_query_options.is_some() && self.results.is_refreshing())
            || (self.dynamic_query.is_some() && self.file_updates.is_none());
        self.cancel_query_options();
        self.keys.clear();
        self.cancel_replacement();
        self.results.version.fetch_add(1, atomic::Ordering::Relaxed);
        self.results.discard_pending();
        self.results.spare_matcher = None;

        if let Some(query) = &mut self.dynamic_query {
            query.suspend();
        }
        if let Some(updates) = &mut self.editor_updates {
            updates.suspend();
        }
        self.poll = None;
        if let Some(updates) = &mut self.file_updates {
            updates.stop();
        }
        self.preview.cancel_loads(None);
        if let Some(preview) = &mut self.custom_preview {
            preview.cancel();
        }
    }

    pub(super) fn cancel_input(&mut self) {
        self.keys.clear();
        self.prompt.cancel_input();
    }

    pub(super) fn clear_filter(&mut self, editor: &Editor) {
        self.prompt.clear(editor);
        if self.handle_prompt_change(false) {
            self.refresh_query_options(editor, true);
        }
    }

    pub(super) fn set_title(&mut self, title: &'static str) {
        self.title = Some(title.into());
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

    fn dynamic_query_pending(&self) -> bool {
        self.dynamic_query
            .as_ref()
            .is_some_and(DynamicQuery::is_pending)
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
        self.dynamic_query = Some(DynamicQuery::new(handler, self.primary_query()));
        self
    }

    pub fn with_default_action(mut self, action: Action) -> Self {
        self.default_action = action;
        self
    }

    /// Install the bindings and selected-item actions for a specific picker.
    pub(crate) fn with_item_actions(
        self,
        keymap: fn(&ComponentKeymaps) -> &Keymap<PickerAction>,
        callback: impl Fn(&mut Context, Option<&T>, PickerItemAction) -> PickerCallbackResult<T, D>
            + 'static,
    ) -> Self {
        self.with_item_data_actions(keymap, move |cx, item, _, action| {
            callback(cx, item, action)
        })
    }

    /// Like [`Self::with_item_actions`], for actions that also need the picker's data,
    /// such as the directory a file explorer is showing.
    pub(crate) fn with_item_data_actions(
        mut self,
        keymap: fn(&ComponentKeymaps) -> &Keymap<PickerAction>,
        callback: impl Fn(&mut Context, Option<&T>, &D, PickerItemAction) -> PickerCallbackResult<T, D>
            + 'static,
    ) -> Self {
        self.keymap = keymap;
        self.item_action_fn = Some(Box::new(callback));
        self
    }

    /// Shortcuts shown together, wrapping between hints when space is limited.
    #[cfg(feature = "git")]
    pub(crate) fn with_help(mut self, help: fn(&ComponentKeymaps) -> Vec<String>) -> Self {
        self.help = Some(help);
        self
    }

    /// Move the cursor by a number of lines, either down (`Forward`) or up (`Backward`)
    pub fn move_by(&mut self, amount: u32, direction: Direction) {
        self.results.cursor = super::list::move_cursor(
            Some(self.results.cursor),
            self.results.matcher.snapshot().matched_item_count(),
            amount,
            direction,
        )
        .unwrap();
    }

    /// Scroll up by one page, keeping the cursor on the same visible row.
    /// Once the first page is visible, move to the first entry and stop.
    pub fn page_up(&mut self) {
        self.page(Direction::Backward);
    }

    /// Scroll down by one page, keeping the cursor on the same visible row.
    /// Once the last page is visible, move to the last entry and stop.
    pub fn page_down(&mut self) {
        self.page(Direction::Forward);
    }

    fn page(&mut self, direction: Direction) {
        // There are no visible result rows to page through.
        if self.completion_height == 0 {
            return;
        }
        let len = self.results.matcher.snapshot().matched_item_count();
        (self.results.cursor, self.results.offset) = page_position(
            self.results.cursor,
            self.results.offset,
            u32::from(self.completion_height),
            len,
            direction,
        );
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
        self.cancel_query_options();
        self.query_needs_refresh = false;
        if self.on_close.is_some() {
            if let Some(updates) = &mut self.file_updates {
                updates.stop();
            }
            // Keep the old filtered snapshot until publication so expansion can
            // resolve the selected node before exposing the unfiltered tree.
            self.refresh_options(options, Arc::new(editor_data));
            self.prompt.clear(editor);
            self.query.parse("");
            let pending = self.results.pending_options.as_mut().unwrap();
            for column in 0..self.columns.iter().filter(|column| column.filter).count() {
                pending.matcher.pattern.reparse(
                    column,
                    "",
                    nucleo::pattern::CaseMatching::Smart,
                    nucleo::pattern::Normalization::Smart,
                    false,
                );
            }
            pending.matcher.tick(10);
            self.publish_options();
        } else {
            self.clear_filter(editor);
            self.results.cursor = 0;
            self.results.offset = 0;
            self.reset_options(options, editor_data);
        }
    }

    fn reset_options(&mut self, options: Vec<T>, editor_data: D) {
        if let Some(updates) = &mut self.file_updates {
            if self.query_options.is_some() {
                updates.query_changed();
            } else {
                updates.stop();
            }
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
        if !self.show_preview {
            self.preview_area = None;
        }
    }

    pub(super) fn prompt_handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if self.replacement_pending() {
            return EventResult::Consumed(None);
        }
        let search_active = !self.replacement_active();
        let result = self.active_prompt_mut().handle_event(event, cx);
        if search_active
            && matches!(result, EventResult::Consumed(_))
            && self.handle_prompt_change(matches!(event, Event::Paste(_)))
        {
            self.refresh_query_options(cx.editor, false);
        }
        EventResult::Consumed(None)
    }

    pub(super) fn refresh_query_options(&mut self, editor: &Editor, preserve_selection: bool) {
        self.publish_options();
        let preserve_selection =
            preserve_selection && self.pending_query.is_none() && !self.results.is_refreshing();
        if self.start_query_options(editor, preserve_selection) {
            return;
        }
        if let Some(source) = &self.query_options {
            let result = source(
                editor,
                &self.primary_query(),
                &self.results.editor_data,
                self.selection().filter(|_| preserve_selection),
            );
            self.apply_query_options(editor, result, preserve_selection);
        }
    }

    /// Reveal an owner-selected item, clearing any filter that could hide it.
    pub(super) fn reveal_query_item(&mut self, editor: &Editor, item: &T) {
        self.cancel_query_options();
        self.publish_options();
        if let Some(source) = &self.query_options {
            let result = source(editor, "", &self.results.editor_data, Some(item));
            self.prompt.clear(editor);
            self.handle_prompt_change(false);
            self.apply_query_options(editor, result, false);
        }
    }

    fn apply_query_options(
        &mut self,
        editor: &Editor,
        result: anyhow::Result<(Vec<T>, D, usize)>,
        preserve_selection: bool,
    ) {
        match result {
            Ok((options, data, cursor)) => {
                if let Some(updates) = &mut self.file_updates {
                    updates.query_changed();
                }
                let data = Arc::new(data);
                let injector = self.prepare_query_options(data.clone(), cursor, preserve_selection);
                for item in options {
                    inject_nucleo_item(&injector, &self.columns, item, &data);
                }
            }
            Err(error) => editor.status_sender().report_blocking(error),
        }
    }

    fn handle_prompt_change(&mut self, is_paste: bool) -> bool {
        // TODO: better track how the pattern has changed
        let line = self.prompt.line();
        let old_query = self.query.parse(line);
        if self.query == old_query {
            return false;
        }
        if self.query_options.is_some() {
            return true;
        }
        // If the query has meaningfully changed, reset the cursor to the top of the results.
        self.results.cursor = 0;
        self.results.offset = 0;
        let primary_changed = &*self.primary_query()
            != old_query
                .get(&self.columns[self.primary_column].name)
                .map(|pattern| &**pattern)
                .unwrap_or_default();
        if primary_changed && self.dynamic_query.is_some() {
            if self.results.pending_options.is_some()
                && let Some(updates) = &mut self.file_updates
            {
                updates.stop();
            }
            self.results.discard_pending();

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
        // Secondary filters update the debounce input without restarting the source.
        if primary_changed
            && self.dynamic_query.is_some()
            && let Some(updates) = &mut self.editor_updates
        {
            updates.task.cancel();
        }
        let query = self.primary_query();
        if let Some(dynamic_query) = &mut self.dynamic_query {
            dynamic_query.schedule(
                DynamicQueryChange { query, is_paste },
                primary_changed.then_some(QueryKind::Replace),
                &self.results.version,
            );
        }
        true
    }

    /// Prepare the selected file's cache and background loads before rendering.
    fn prepare_preview(
        &mut self,
        editor: &Editor,
        jobs: &crate::job::Jobs,
        image_picker: Option<&ratatui_image::picker::Picker>,
        image_size: Size,
    ) {
        let snapshot = self.results.matcher.snapshot();
        let location = snapshot
            .get_matched_item(self.results.cursor)
            .and_then(|current| (self.file_fn.as_ref()?)(editor, current.data));
        let Some(location) = location else {
            self.preview.cancel_loads(None);
            self.preview_scroll = FileScroll::default();
            return;
        };
        self.preview_scroll.select(&location);
        self.preview
            .prepare::<T, D>(editor, location.0, jobs, image_picker, image_size);
    }

    fn get_preview<'picker, 'editor>(
        &'picker mut self,
        editor: &'editor Editor,
    ) -> Option<SelectedPreview<'picker, 'editor>> {
        let location = self.preview_scroll.location()?;
        let (preview, range) = self.preview.cached(editor, location)?;
        Some((preview, range, &mut self.preview_scroll))
    }

    fn frame(
        &self,
        theme: &theme::Theme,
        border_style: editor_config::BorderStyle,
    ) -> tui::widgets::Block<'static> {
        if self.on_close.is_some() {
            tui::widgets::Block::new()
                .borders(tui::widgets::Borders::RIGHT)
                .border_set(tui::symbols::border::PLAIN)
                .border_style(theme.get("ui.window"))
                .title(self.title.clone().unwrap_or_default())
        } else {
            panel::bordered(theme, border_style)
        }
    }

    fn render_picker(&mut self, area: Rect, surface: &mut Surface, cx: &mut DrawContext) {
        let snapshot = self.results.matcher.snapshot();
        let text_style = cx.editor.theme.get("ui.text");
        let selected = cx
            .editor
            .theme
            .get(if self.on_close.is_some() && !cx.is_cursor_owner {
                "ui.text.inactive"
            } else {
                "ui.text.focus"
            });
        let highlight_style = cx
            .editor
            .theme
            .get("ui.picker.match")
            .add_modifier(Modifier::BOLD);

        // -- Render the frame:
        // clear area
        let background = cx.editor.theme.get("ui.background");
        cx.hyperlinks.borrow_mut().clear(area);
        surface.clear_with(area, background);

        let mut block = self.frame(&cx.editor.theme, cx.editor.config().border_style);
        if let Some(title) = &self.title {
            let title_style = if self.on_close.is_some() && cx.is_cursor_owner {
                text_style.add_modifier(Modifier::BOLD)
            } else {
                text_style
            };
            if self.on_close.is_none() {
                block = block.title(title.clone());
            }
            block = block.title_style(title_style);
        }
        let inner = block.inner(area);
        block.render(area, surface);
        self.render_help(area, surface, cx);

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
            if self.dynamic_query_pending()
                || self.matching
                || self.results.matcher.active_injectors() > 0
            {
                "(running) "
            } else {
                ""
            },
            snapshot.matched_item_count(),
            if self.dynamic_query_pending()
                || self.results.source.get() != source::State::Complete
                || self.results.is_refreshing()
                || self
                    .editor_updates
                    .as_ref()
                    .is_some_and(|updates| updates.incomplete)
            {
                "?".to_owned()
            } else {
                snapshot.item_count().to_string()
            },
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
        if self.on_close.is_none() {
            panel::top_border(sep_style, cx.editor.config().border_style)
                .render(input_areas.search_separator, surface);
        }
        if let Some(status) = self
            .editor_updates
            .as_ref()
            .and_then(|updates| updates.status.as_deref())
            .or_else(|| match self.results.source.get() {
                source::State::Failed => Some("Results incomplete; source failed"),
                source::State::Paused => Some("Results incomplete; loading stopped"),
                source::State::Loading if self.results.matcher.active_injectors() == 0 => {
                    Some("Results incomplete; source did not finish")
                }
                _ => None,
            })
        {
            // Source status uses the existing separator so it cannot shrink the query
            // or consume result rows, including when a preview narrows the picker.
            Paragraph::new(status)
                .style(cx.editor.theme.get("ui.text.inactive"))
                .render(
                    input_areas.search_separator.clip_left(1).clip_right(1),
                    surface,
                );
        }
        if let Some(area) = input_areas.replacement_separator {
            panel::top_border(sep_style, cx.editor.config().border_style).render(area, surface);
        }

        let rows = inner.height.saturating_sub(self.header_height()) as u32;
        self.completion_height = rows as u16;
        let rows = rows.max(1);
        self.results.offset = visible_offset(
            self.results.cursor,
            self.results.offset,
            rows,
            snapshot.matched_item_count(),
        );
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
        let source_pattern = self.query_options.as_ref().map(|_| {
            nucleo::pattern::Pattern::parse(
                &self.primary_query(),
                nucleo::pattern::CaseMatching::Smart,
                nucleo::pattern::Normalization::Smart,
            )
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
                    if matcher
                        .indices(
                            source_pattern.as_ref().unwrap_or_else(|| {
                                snapshot.pattern().column_pattern(matcher_index)
                            }),
                            item.matcher_columns[matcher_index].slice(..),
                            &mut indices,
                        )
                        .is_none()
                    {
                        indices.clear();
                    }
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
                                span.style.patch(highlight_style)
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

        let marker = " > ";
        let mut options: Vec<_> = options.collect();
        if self.truncate_start {
            tui::table::truncate_rows_start(&mut options, inner, &self.widths, 1, marker);
        }
        let mut table = Table::new(options, &self.widths)
            .style(text_style)
            .row_highlight_style(selected)
            .highlight_symbol(marker)
            .highlight_spacing(tui::widgets::HighlightSpacing::Always)
            .column_spacing(1)
            .flex(tui::layout::Flex::Legacy);

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
                .style(header_style)
                .into(),
            );
        }

        use tui::widgets::TableState;

        let mut table_state = TableState::default().with_selected(Some(cursor as usize));

        StatefulWidget::render(table, inner, surface, &mut table_state);
    }

    fn render_preview(&mut self, area: Rect, surface: &mut Surface, cx: &mut DrawContext) {
        let background = cx.editor.theme.get("ui.background");
        cx.hyperlinks.borrow_mut().clear(area);
        surface.clear_with(area, background);

        let block = panel::horizontally_padded(&cx.editor.theme, cx.editor.config().border_style);
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.is_empty() {
            self.preview_area = None;
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
        if let Some((preview, range, scroll)) = self.get_preview(cx.editor) {
            render_preview_content(preview, range, scroll, inner, surface, cx);
        }
    }

    pub(super) fn execute_actions(
        &mut self,
        actions: &[PickerAction],
        ctx: &mut Context,
    ) -> EventResult {
        let close_fn = |picker: &mut Self, after: Option<compositor::Callback>| {
            if let Some(on_close) = picker.on_close {
                return EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                    on_close(compositor, cx);
                    if let Some(after) = after {
                        after(compositor, cx);
                    }
                })));
            }
            picker.suspend();
            // if the picker is very large don't store it as last_picker to avoid
            // excessive memory consumption
            let callback: compositor::Callback =
                if picker.results.matcher.snapshot().item_count() > 1_000_000 {
                    Box::new(|compositor: &mut Compositor, _ctx| {
                        // remove the layer
                        compositor.pop();
                    })
                } else {
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
        // Resume its query before any action can use the previous published rows.
        if self.query_needs_refresh {
            self.refresh_query_if_changed(ctx.editor);
        }
        self.poll_query_options(ctx.editor);
        self.refresh_from_editor(ctx.editor);
        if self.query_options.is_some() {
            self.results.tick(10);
        }
        if self.on_close.is_some() {
            if let Some(pending) = &mut self.results.pending_options {
                pending.matcher.tick(10);
            }
            self.publish_options();
        }
        for &action in actions {
            if self.replacement_pending()
                && !matches!(
                    action,
                    PickerAction::Cancel
                        | PickerAction::MoveUp
                        | PickerAction::MoveDown
                        | PickerAction::PageUp
                        | PickerAction::PageDown
                        | PickerAction::MoveStart
                        | PickerAction::MoveEnd
                        | PickerAction::TogglePreview
                )
            {
                return EventResult::Consumed(None);
            }
            // Keep navigation available while replacements load or a resumed
            // picker waits for its old matcher before checking the current source.
            if (self.query_needs_refresh
                || self.pending_query.is_some()
                || self.results.is_refreshing()
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
                PickerAction::ToggleLoading => {
                    if let Some(updates) = &mut self.editor_updates
                        && updates.paged
                    {
                        updates.toggle_loading = true;
                        self.redraw.request();
                    }
                }
                PickerAction::ShowKeybindings => {
                    return EventResult::Consumed(Some(Box::new(ui::keybindings::show)))
                }
                PickerAction::InspectKey => {
                    return EventResult::Consumed(Some(Box::new(ui::keybindings::inspect)))
                }
                PickerAction::HistoryPrevious | PickerAction::HistoryNext => {
                    let prompt_action = if action == PickerAction::HistoryPrevious {
                        crate::keymap::component::PromptAction::HistoryPrevious
                    } else {
                        crate::keymap::component::PromptAction::HistoryNext
                    };
                    self.active_prompt_mut()
                        .execute_actions(&[prompt_action], ctx);
                    if self.handle_prompt_change(false) {
                        self.refresh_query_options(ctx.editor, false);
                    }
                }
                PickerAction::Cancel => {
                    if self.replacement_pending() {
                        ctx.editor.clear_status();
                    }
                    return close_fn(self, None);
                }
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
                        let result = (self.callback_fn)(
                            ctx,
                            option,
                            &self.results.editor_data,
                            self.default_action,
                        );
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
                        if self.handle_prompt_change(true) {
                            self.refresh_query_options(ctx.editor, false);
                        }
                    } else {
                        let callback_result =
                            self.selection()
                                .map_or(PickerCallbackResult::Close, |option| {
                                    (self.callback_fn)(
                                        ctx,
                                        option,
                                        &self.results.editor_data,
                                        self.default_action,
                                    )
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
                            (self.callback_fn)(ctx, option, &self.results.editor_data, action)
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
                    if self.preview_area.is_none()
                        && matches!(
                            action,
                            PickerAction::PreviewScrollUp | PickerAction::PreviewScrollDown
                        )
                    {
                        continue;
                    }
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
                    match action {
                        PickerAction::PreviewScrollUp => self.preview_scroll.scroll.scroll(false),
                        PickerAction::PreviewScrollDown => self.preview_scroll.scroll.scroll(true),
                        _ => {}
                    }
                }
                PickerAction::TogglePreview => {
                    self.toggle_preview();
                }
                PickerAction::Item(action) => {
                    if let Some(handler) = &self.item_action_fn {
                        let data = self.results.editor_data.clone();
                        let result = handler(ctx, self.selection(), &data, action);
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

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> Picker<T, D> {
    fn preview_visible(&self, area: Rect) -> bool {
        self.show_preview
            && (self.file_fn.is_some() || self.custom_preview.is_some())
            && area.width > MIN_AREA_WIDTH_FOR_PREVIEW
    }
}

impl<I: 'static + Send + Sync, D: 'static + Send + Sync> Component for Picker<I, D> {
    fn keymap_contexts(&self, cx: &Context) -> Vec<crate::keymap::inspect::Context> {
        use crate::keymap::inspect;
        let maps = &cx.config.current.component_keys;
        let keys = (self.keymap)(maps);
        let filtered = inspect::filter(keys, |action| self.action_available(maps, *action));
        let mut context = inspect::Context::new(
            "Picker",
            inspect::component(maps.picker_table(keys), &filtered),
        );
        context.pending = self.keys.pending().to_vec();
        let mut prompt = self.query_contexts(cx);
        if self.active_prompt().has_pending_keys() {
            prompt.push(context);
        } else {
            prompt.insert(0, context);
        }
        prompt
    }

    fn has_pending_render(&self) -> bool {
        self.matching
            || self.query_needs_refresh
            || self.pending_query.is_some()
            || (self.on_close.is_some() && self.results.is_refreshing())
    }

    fn update(&mut self, area: Rect, cx: &mut Context) {
        let render_preview = self.preview_visible(area);

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

        self.poll_query_options(cx.editor);
        self.poll_replacement(cx);
        self.refresh_files(cx.editor);
        self.refresh_query_if_changed(cx.editor);
        self.refresh_from_editor(cx.editor);

        self.publish_options();
        let status = self.results.tick(10);
        let snapshot = self.results.matcher.snapshot();
        if status.changed {
            self.results.cursor = self
                .results
                .cursor
                .min(snapshot.matched_item_count().saturating_sub(1))
        }

        self.matching = status.running
            || self
                .results
                .pending_options
                .as_ref()
                .is_some_and(|pending| pending.matcher.active_injectors() == 0);
        let (_, preview_area) = split_picker_area(area, render_preview);
        self.preview_area = preview_area;
        let inner = preview_area
            .map(|area| {
                panel::horizontally_padded(&cx.editor.theme, cx.editor.config().border_style)
                    .inner(area)
            })
            .filter(|area| !area.is_empty());
        if let Some(inner) = inner {
            if let Some(preview) = &mut self.custom_preview {
                let item = self
                    .results
                    .matcher
                    .snapshot()
                    .get_matched_item(self.results.cursor)
                    .map(|item| item.data);
                preview.update(item, inner, cx);
            } else {
                let size = Size::new(inner.width, inner.height.saturating_sub(2).max(1));
                self.prepare_preview(cx.editor, cx.jobs, cx.image_picker, size);
            }
        } else {
            self.preview.cancel_loads(None);
            if let Some(preview) = &mut self.custom_preview {
                preview.cancel();
            }
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut DrawContext) {
        // +---------+ +---------+
        // |prompt   | |preview  |
        // +---------+ |         |
        // |picker   | |         |
        // |         | |         |
        // +---------+ +---------+

        let render_preview = self.preview_visible(area);
        let (picker_area, preview_area) = split_picker_area(area, render_preview);
        self.preview_area = preview_area;
        self.render_picker(picker_area, surface, cx);

        if let Some(preview_area) = preview_area {
            self.render_preview(preview_area, surface, cx);
        }
        self.render_action_menu(area, surface, cx);
    }

    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending() || self.active_prompt().has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut Context) -> EventResult {
        let key_event = match event {
            Event::Key(event) => *event,
            Event::Paste(..) => {
                self.keys.clear();
                return self.prompt_handle_event(event, ctx);
            }
            Event::Resize(..) => {
                self.preview_area = None;
                return EventResult::Consumed(None);
            }
            // Picker is a modal and should consume mouse events so clicks don't fall
            // through to the editor underneath
            Event::Mouse(mouse) => {
                use ui_core::input::MouseEventKind;
                if self.preview_area.is_some_and(|area| {
                    mouse.column >= area.left()
                        && mouse.column < area.right()
                        && mouse.row >= area.top()
                        && mouse.row < area.bottom()
                }) {
                    let action = match mouse.kind {
                        MouseEventKind::ScrollDown => Some(PickerAction::PreviewScrollDown),
                        MouseEventKind::ScrollUp => Some(PickerAction::PreviewScrollUp),
                        _ => None,
                    };
                    if let Some(action) = action {
                        return self.execute_actions(&[action], ctx);
                    }
                }
                return EventResult::Consumed(None);
            }
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

    fn cursor(&self, area: Rect, cx: &DrawContext) -> Cursor {
        let editor = cx.editor;
        let render_preview = self.show_preview
            && (self.file_fn.is_some() || self.custom_preview.is_some())
            && area.width > MIN_AREA_WIDTH_FOR_PREVIEW;
        let (picker_area, _) = split_picker_area(area, render_preview);
        let inner = self
            .frame(&editor.theme, editor.config().border_style)
            .inner(picker_area);
        let areas = split_picker_input_area(inner, self.replacement_visible());
        let area = if self.replacement_active() {
            areas.replacement.unwrap().clip_right(1)
        } else {
            areas.search
        };
        self.active_prompt().cursor(area.clip_left(1), cx)
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

type QueryOptions<T, D> =
    Box<dyn Fn(&Editor, &str, &D, Option<&T>) -> anyhow::Result<(Vec<T>, D, usize)>>;

type PickerCallback<T, D> = Box<dyn Fn(&mut Context, &T, &D, Action) -> PickerCallbackResult<T, D>>;
type PickerItemCallback<T, D> =
    Box<dyn Fn(&mut Context, Option<&T>, &D, PickerItemAction) -> PickerCallbackResult<T, D>>;

/// First visible result for `rows` rows, keeping the cursor in view with minimal scrolling.
///
/// Publication can move selection toward the end of a reordered result set, so the
/// viewport is also filled from above instead of leaving most rows blank.
fn visible_offset(cursor: u32, offset: u32, rows: u32, len: u32) -> u32 {
    let rows = rows.max(1);
    super::list::scroll_offset(
        cursor as usize,
        offset.min(len.saturating_sub(rows)) as usize,
        rows as usize,
    ) as u32
}

/// Cursor and offset after paging, as `(cursor, offset)`.
///
/// The viewport and cursor move together so the cursor keeps its visible row. When less
/// than a full page remains, only the remaining distance is scrolled. Once the first or
/// last page is visible, paging reaches the corresponding endpoint and stops.
fn page_position(
    cursor: u32,
    offset: u32,
    rows: u32,
    len: u32,
    direction: Direction,
) -> (u32, u32) {
    if len == 0 {
        return (cursor, offset);
    }
    let rows = rows.max(1);
    let cursor = cursor.min(len - 1);
    let offset = visible_offset(cursor, offset, rows, len);
    let row = cursor - offset;
    let last_page = len.saturating_sub(rows);
    let offset = match direction {
        Direction::Forward if offset < last_page => offset + rows.min(last_page - offset),
        Direction::Forward => return (len - 1, offset),
        Direction::Backward if offset > 0 => offset - rows.min(offset),
        Direction::Backward => return (0, offset),
    };
    (offset + row, offset)
}

#[cfg(test)]
mod tests {
    use super::{page_position, split_picker_area, visible_column_widths, visible_offset, Column};
    use crate::ui::image::image_preview_layout;
    use editor_core::movement::Direction::{Backward, Forward};
    use tui::layout::{Constraint, Size};
    use view::graphics::Rect;

    #[test]
    fn paging_keeps_the_cursor_on_its_visible_row() {
        // 10 rows, cursor on the fourth visible row of the first page.
        assert_eq!(page_position(3, 0, 10, 100, Forward), (13, 10));
        assert_eq!(page_position(13, 10, 10, 100, Forward), (23, 20));
        assert_eq!(page_position(23, 20, 10, 100, Backward), (13, 10));
        // Paging uses the viewport, not the cursor, so a mid-page cursor never
        // first jumps to the bottom of the current page.
        assert_eq!(page_position(17, 12, 10, 100, Forward), (27, 22));
    }

    #[test]
    fn paging_near_the_end_scrolls_only_the_remaining_distance() {
        // 25 results, 10 rows: only 5 more rows exist below the second page.
        assert_eq!(page_position(14, 10, 10, 25, Forward), (19, 15));
        assert_eq!(page_position(16, 15, 10, 25, Backward), (6, 5));
        assert_eq!(page_position(6, 5, 10, 25, Backward), (1, 0));
        // The cursor never moves up while paging down.
        let (mut cursor, mut offset) = (4, 0);
        while offset < 15 {
            let next = page_position(cursor, offset, 10, 25, Forward);
            assert!(next.0 > cursor);
            assert_eq!(next.0 - next.1, cursor - offset);
            (cursor, offset) = next;
        }
    }

    #[test]
    fn paging_on_the_outer_pages_reaches_the_endpoints_and_stops() {
        assert_eq!(page_position(19, 15, 10, 25, Forward), (24, 15));
        assert_eq!(page_position(24, 15, 10, 25, Forward), (24, 15));
        assert_eq!(page_position(4, 0, 10, 25, Backward), (0, 0));
        assert_eq!(page_position(0, 0, 10, 25, Backward), (0, 0));
    }

    #[test]
    fn paging_one_result_beyond_a_full_page_reaches_the_last_result() {
        assert_eq!(page_position(0, 0, 10, 11, Forward), (1, 1));
        assert_eq!(page_position(1, 1, 10, 11, Forward), (10, 1));
        assert_eq!(page_position(10, 1, 10, 11, Forward), (10, 1));
    }

    #[test]
    fn paging_a_single_page_moves_to_its_ends() {
        assert_eq!(page_position(3, 0, 10, 8, Forward), (7, 0));
        assert_eq!(page_position(3, 0, 10, 8, Backward), (0, 0));
        assert_eq!(page_position(0, 0, 10, 0, Forward), (0, 0));
    }

    #[test]
    fn paging_repairs_a_stale_viewport_before_moving() {
        // Cursor below the stored viewport: rendering would show it on the last row.
        assert_eq!(visible_offset(30, 0, 10, 100), 21);
        assert_eq!(page_position(30, 0, 10, 100, Forward), (40, 31));
        // Results shrank under the viewport: offset 2 is now the last page.
        assert_eq!(page_position(4, 90, 10, 12, Forward), (11, 2));
    }

    #[test]
    fn paged_positions_are_stable_when_rendered() {
        for len in [1, 9, 10, 11, 25, 100] {
            for start in 0..len {
                for direction in [Forward, Backward] {
                    let offset = visible_offset(start, 0, 10, len);
                    let (mut cursor, mut offset) = (start, offset);
                    loop {
                        let (next, next_offset) = page_position(cursor, offset, 10, len, direction);
                        assert!(next < len);
                        assert_eq!(visible_offset(next, next_offset, 10, len), next_offset);
                        match direction {
                            Forward => assert!(next >= cursor),
                            Backward => assert!(next <= cursor),
                        }
                        if next == cursor {
                            assert_eq!(next, if direction == Forward { len - 1 } else { 0 });
                            break;
                        }
                        if next_offset != offset {
                            assert_eq!(next - next_offset, cursor - offset);
                        }
                        (cursor, offset) = (next, next_offset);
                    }
                }
            }
        }
    }

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

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_preserves_cursor_and_blocks_row_actions_until_matching_finishes(
    ) -> anyhow::Result<()> {
        use super::*;
        use crate::{config::Config as AppConfig, job::Jobs};
        use std::{cell::RefCell, rc::Rc, time::Duration};

        let config = AppConfig::default();
        let mut app = super::test::application(false)?;
        let mut jobs = Jobs::new();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
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
        picker.update_and_render(area, &mut Surface::empty(area), &mut cx);
        let prefix = Event::Key("C-x".parse().unwrap());
        let remove = Event::Key("d".parse().unwrap());
        let populate = Event::Key("q".parse().unwrap());
        for event in [&accept, &prefix, &remove, &prefix, &populate] {
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
                picker.update_and_render(area, &mut Surface::empty(area), &mut cx);
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(picker.results.cursor, 1);
        picker.handle_event(&prefix, &mut cx);
        assert!(matches!(
            picker.handle_event(&remove, &mut cx),
            EventResult::Consumed(None)
        ));
        assert_eq!(*removed.borrow(), [2]);
        picker.handle_event(&prefix, &mut cx);
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

    #[tokio::test(flavor = "multi_thread")]
    async fn resumed_picker_blocks_stale_row_actions_while_matcher_is_busy() -> anyhow::Result<()> {
        use super::*;
        use crate::{config::Config as AppConfig, job::Jobs};
        use std::{
            cell::RefCell,
            rc::Rc,
            sync::{atomic::AtomicBool, mpsc, Mutex},
            time::Duration,
        };

        let config = AppConfig::default();
        let mut app = super::test::application(false)?;
        let mut jobs = Jobs::new();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
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
        for key in ["C-x", "q", "ret", "C-x", "d"] {
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
                picker.update_and_render(area, &mut Surface::empty(area), &mut cx);
                if !picker.results.is_refreshing()
                    && !picker.editor_updates.as_ref().unwrap().resumed
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        for key in ["C-x", "q", "ret", "C-x", "d"] {
            picker.handle_event(&Event::Key(key.parse().unwrap()), &mut cx);
        }
        assert_eq!(cx.editor.quicklist.entries(), expected.as_slice());
        assert_eq!(*acted_on.borrow(), [4, 4]);
        assert!(app.close().await.is_empty());
        Ok(())
    }
}
