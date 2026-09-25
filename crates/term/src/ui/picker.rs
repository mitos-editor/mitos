mod handlers;
mod query;

use crate::ui::image::{cached_image, render_image, PreparedImage};
use crate::{
    alt,
    compositor::{self, Component, Compositor, Context, Event, EventResult},
    ctrl, key, shift,
    ui::{
        self,
        document::{render_document, LinePos, TextRenderer},
        panel,
        picker::query::PickerQuery,
        text_decorations::DecorationManager,
        EditorView,
    },
};
use event::AsyncHook;
use futures_util::future::BoxFuture;
use nucleo::pattern::{CaseMatching, Normalization};
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
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{self, AtomicUsize},
        Arc,
    },
};

use crate::ui::{Prompt, PromptEvent};
use editor_core::{
    char_idx_at_visual_offset, fuzzy::MATCHER, movement::Direction,
    text_annotations::TextAnnotations, unicode::segmentation::UnicodeSegmentation, Position,
};
use view::{
    editor::Action,
    graphics::{CursorKind, Modifier, Rect},
    icons::ICONS,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    view::ViewPosition,
    Document, DocumentId, Editor,
};

use self::handlers::{
    spawn_image_preview, DynamicQueryChange, DynamicQueryHandler, ImagePreviewTask,
    PreviewHighlightHandler,
};

pub const ID: &str = "picker";

pub const MIN_AREA_WIDTH_FOR_PREVIEW: u16 = 72;
/// Biggest file size to preview in bytes
pub const MAX_FILE_SIZE_FOR_PREVIEW: u64 = 10 * 1024 * 1024;

pub(crate) fn split_picker_area(area: Rect, show_preview: bool) -> (Rect, Option<Rect>) {
    if show_preview {
        let [picker, preview] =
            Layout::horizontal([Constraint::Length(area.width / 2), Constraint::Min(0)])
                .areas(area);
        (picker, Some(preview))
    } else {
        (area, None)
    }
}

#[derive(PartialEq, Eq, Hash)]
pub enum PathOrId<'a> {
    Id(DocumentId),
    Path(&'a Path),
}

impl<'a> From<&'a Path> for PathOrId<'a> {
    fn from(path: &'a Path) -> Self {
        Self::Path(path)
    }
}

impl From<DocumentId> for PathOrId<'_> {
    fn from(v: DocumentId) -> Self {
        Self::Id(v)
    }
}

type FileCallback<T> = Box<dyn for<'a> Fn(&'a Editor, &'a T) -> Option<FileLocation<'a>>>;
type QuicklistCallback<T> = Box<dyn Fn(&Editor, &T) -> Option<QuicklistEntry>>;

/// File path and range of lines (used to align and highlight lines)
pub type FileLocation<'a> = (PathOrId<'a>, Option<(usize, usize)>);

pub enum CachedPreview {
    Document(Box<Document>),
    Directory(Vec<(PathBuf, bool)>),
    Image(Box<ImagePreview>),
    Binary,
    LargeFile,
    NotFound,
}

pub enum ImagePreview {
    Loading {
        size: Size,
        request: Arc<()>,
    },
    Ready {
        size: Size,
        image: Arc<PreparedImage>,
    },
    Failed {
        size: Size,
    },
}

impl ImagePreview {
    fn size(&self) -> Size {
        match self {
            Self::Loading { size, .. } | Self::Ready { size, .. } | Self::Failed { size } => *size,
        }
    }
}

// We don't store this enum in the cache so as to avoid lifetime constraints
// from borrowing a document already opened in the editor.
pub enum Preview<'picker, 'editor> {
    Cached(&'picker mut CachedPreview),
    EditorDocument(&'editor Document),
}

impl Preview<'_, '_> {
    fn document(&self) -> Option<&Document> {
        match self {
            Preview::EditorDocument(doc) => Some(doc),
            Preview::Cached(CachedPreview::Document(doc)) => Some(doc),
            _ => None,
        }
    }

    fn dir_content(&self) -> Option<&Vec<(PathBuf, bool)>> {
        match self {
            Preview::Cached(CachedPreview::Directory(dir_content)) => Some(dir_content),
            _ => None,
        }
    }

    fn image(&mut self) -> Option<&mut ImagePreview> {
        match self {
            Preview::Cached(CachedPreview::Image(image)) => Some(image.as_mut()),
            _ => None,
        }
    }

    /// Alternate text to show for the preview.
    fn placeholder(&self) -> &str {
        match self {
            Self::EditorDocument(_) => "<Invalid file location>",
            Self::Cached(preview) => match &**preview {
                CachedPreview::Document(_) => "<Invalid file location>",
                CachedPreview::Directory(_) => "<Invalid directory location>",
                CachedPreview::Image(image) => match &**image {
                    ImagePreview::Loading { .. } => "<Loading image>",
                    ImagePreview::Ready { .. } => "<Rendering image>",
                    ImagePreview::Failed { .. } => "<Image preview unavailable>",
                },
                CachedPreview::Binary => "<Binary file>",
                CachedPreview::LargeFile => "<File too large to preview>",
                CachedPreview::NotFound => "<File not found>",
            },
        }
    }
}

pub(crate) struct FilePreview {
    preview_cache: HashMap<Arc<Path>, CachedPreview>,
    read_buffer: Vec<u8>,
    image_task: Option<ImagePreviewTask>,
}

impl Default for FilePreview {
    fn default() -> Self {
        Self {
            preview_cache: HashMap::new(),
            read_buffer: Vec::with_capacity(1024),
            image_task: None,
        }
    }
}

impl FilePreview {
    fn cancel_image_load(&mut self, location: Option<(&Path, Size)>) {
        if self
            .image_task
            .as_ref()
            .is_some_and(|task| location != Some((task.path.as_ref(), task.size)))
        {
            let task = self.image_task.take().unwrap();
            if matches!(self.preview_cache.get(&task.path), Some(CachedPreview::Image(image))
                if matches!(image.as_ref(), ImagePreview::Loading { .. }))
            {
                self.preview_cache.remove(&task.path);
            }
        }
    }

    pub(crate) fn render(
        &mut self,
        area: Rect,
        surface: &mut Surface,
        cx: &Context,
        location: Option<FileLocation<'_>>,
    ) {
        let background = cx.editor.theme.get("ui.background");
        surface.clear_with(area, background);

        let block = panel::horizontally_padded(&cx.editor.theme);
        let inner = block.inner(area);
        block.render(area, surface);

        let Some(location) = location else {
            return;
        };
        let Some((preview, range)) = self.get::<(), ()>(
            cx.editor,
            location,
            None,
            None,
            Size::new(inner.width, inner.height),
        ) else {
            return;
        };
        render_preview_content(preview, range, area, inner, surface, cx);
    }

    fn clear(&mut self) {
        self.image_task = None;
        self.preview_cache.clear();
        self.read_buffer.clear();
    }

    fn get<'preview, 'editor, T: 'static + Send + Sync, D: 'static + Send + Sync>(
        &'preview mut self,
        editor: &'editor Editor,
        (path_or_id, range): FileLocation<'_>,
        preview_highlight_handler: Option<&Sender<Arc<Path>>>,
        image_picker: Option<&ratatui_image::picker::Picker>,
        image_size: Size,
    ) -> Option<(Preview<'preview, 'editor>, Option<(usize, usize)>)> {
        let path_or_id = match path_or_id {
            PathOrId::Id(id) => {
                let doc = editor.documents.get(&id)?;
                if doc.is_binary() {
                    doc.path().map_or(PathOrId::Id(id), PathOrId::Path)
                } else {
                    PathOrId::Id(id)
                }
            }
            path => path,
        };
        self.cancel_image_load(match path_or_id {
            PathOrId::Path(path) => Some((path, image_size)),
            PathOrId::Id(_) => None,
        });
        match path_or_id {
            PathOrId::Path(path) => {
                if let Some(doc) = editor.document_by_path(path).filter(|doc| !doc.is_binary()) {
                    return Some((Preview::EditorDocument(doc), range));
                }

                if matches!(self.preview_cache.get(path), Some(CachedPreview::Image(image))
                    if image.size() != image_size || matches!(image.as_ref(), ImagePreview::Ready { image, .. }
                        if image_picker.is_some_and(|picker| !image.is_current(path, picker, image_size))))
                {
                    self.preview_cache.remove(path);
                }
                if self.preview_cache.contains_key(path) {
                    let (path, preview) = self.preview_cache.get_key_value(path).unwrap();
                    let path = Arc::clone(path);
                    if matches!(preview, CachedPreview::Document(doc) if doc.syntax().is_none())
                        && let Some(handler) = preview_highlight_handler
                    {
                        event::send_blocking(handler, path.clone());
                    }
                    let preview = self.preview_cache.get_mut(&path).unwrap();
                    return Some((Preview::Cached(preview), range));
                }

                let path: Arc<Path> = path.into();
                if let Some(image) =
                    image_picker.and_then(|picker| cached_image(&path, picker, image_size))
                {
                    // Other images remain available in the shared, bounded cache.
                    self.preview_cache
                        .retain(|_, preview| !matches!(preview, CachedPreview::Image(_)));
                    self.preview_cache.insert(
                        path.clone(),
                        CachedPreview::Image(Box::new(ImagePreview::Ready {
                            size: image_size,
                            image,
                        })),
                    );
                    return Some((
                        Preview::Cached(self.preview_cache.get_mut(&path).unwrap()),
                        range,
                    ));
                }
                let preview = std::fs::metadata(&path)
                    .and_then(|metadata| {
                        if metadata.is_dir() {
                            let files =
                                super::directory_content(&path, &editor.config().file_explorer)?;
                            Ok(CachedPreview::Directory(files))
                        } else if metadata.is_file() {
                            if metadata.len() > MAX_FILE_SIZE_FOR_PREVIEW {
                                return Ok(CachedPreview::LargeFile);
                            }
                            let is_binary = std::fs::File::open(&path).and_then(|file| {
                                let n = file.take(1024).read_to_end(&mut self.read_buffer)?;
                                let bytes = &self.read_buffer[..n];
                                let is_binary =
                                    crate::is_binary(bytes) || image::guess_format(bytes).is_ok();
                                self.read_buffer.clear();
                                Ok(is_binary)
                            })?;
                            if is_binary {
                                return Ok(if image_picker.is_some() {
                                    CachedPreview::Image(Box::new(ImagePreview::Loading {
                                        size: image_size,
                                        request: Arc::new(()),
                                    }))
                                } else {
                                    CachedPreview::Binary
                                });
                            }
                            let mut doc = Document::open(
                                &path,
                                None,
                                false,
                                editor.config.clone(),
                                editor.syn_loader.clone(),
                            )
                            .or(Err(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "Cannot open document",
                            )))?;
                            let loader = editor.syn_loader.load();
                            let language_config = doc.detect_language_config(&loader);
                            if let Some(handler) = preview_highlight_handler {
                                if language_config.is_some() {
                                    event::send_blocking(handler, path.clone());
                                }
                                doc.language = language_config;
                            } else {
                                doc.set_language(language_config, &loader);
                            }
                            Ok(CachedPreview::Document(Box::new(doc)))
                        } else {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "Neither a dir, nor a file",
                            ))
                        }
                    })
                    .unwrap_or(CachedPreview::NotFound);
                let request = match &preview {
                    CachedPreview::Image(image) => match image.as_ref() {
                        ImagePreview::Loading { request, .. } => Some(request.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                if request.is_some() {
                    // Decoded images and their terminal encodings can be much larger than the
                    // files on disk. Keep only the active image preview while retaining the
                    // existing cache behavior for text documents and directories.
                    self.preview_cache
                        .retain(|_, preview| !matches!(preview, CachedPreview::Image(_)));
                }
                self.preview_cache.insert(path.clone(), preview);
                if let Some(request) = request {
                    self.image_task = Some(spawn_image_preview::<T, D>(
                        path.clone(),
                        image_picker.unwrap().clone(),
                        image_size,
                        request,
                    ));
                }
                Some((
                    Preview::Cached(self.preview_cache.get_mut(&path).unwrap()),
                    range,
                ))
            }
            PathOrId::Id(id) => {
                let doc = editor.documents.get(&id).unwrap();
                Some((Preview::EditorDocument(doc), range))
            }
        }
    }
}

fn render_preview_content(
    mut preview: Preview<'_, '_>,
    range: Option<(usize, usize)>,
    area: Rect,
    inner: Rect,
    surface: &mut Surface,
    cx: &Context,
) {
    let text_style = cx.editor.theme.get("ui.text");
    let directory_style = cx.editor.theme.get("ui.text.directory");
    if let Some(image) = preview.image() {
        match image {
            ImagePreview::Loading { .. } => {
                render_preview_placeholder(surface, inner, "<Loading image>", text_style);
            }
            ImagePreview::Ready { image, .. } => {
                render_image(&image.protocol, &image.details, inner, surface, text_style);
            }
            ImagePreview::Failed { .. } => {
                render_preview_placeholder(
                    surface,
                    inner,
                    "<Image preview unavailable>",
                    text_style,
                );
            }
        }
        return;
    }

    let doc = match preview.document() {
        Some(doc)
            if range.is_none_or(|(start, end)| start <= end && end <= doc.text().len_lines()) =>
        {
            doc
        }
        _ => {
            if let Some(dir_content) = preview.dir_content() {
                for (i, (path, is_dir)) in
                    dir_content.iter().take(inner.height as usize).enumerate()
                {
                    let name = path
                        .file_name()
                        .map_or_else(|| Cow::Borrowed(".."), |name| name.to_string_lossy());

                    if cx.editor.config().icons {
                        let icons = ICONS.load();
                        let icon = if *is_dir {
                            icons.fs().directory().map(|directory_icons| {
                                directory_icons.get_with_style_or_default(
                                    &name,
                                    path.file_name().is_none(),
                                    &cx.editor.theme,
                                    directory_style,
                                )
                            })
                        } else {
                            icons.fs().file().map(|file_icons| {
                                file_icons.get_with_style_or_default(path, &cx.editor.theme)
                            })
                        };

                        if let Some(icon) = icon {
                            surface.set_stringn(
                                inner.x,
                                inner.y + i as u16,
                                icon.glyph(),
                                inner.width as usize,
                                icon.style(),
                            );
                            let suffix = if *is_dir { "/" } else { "" };
                            surface.set_stringn(
                                inner.x + icon.width(),
                                inner.y + i as u16,
                                format!("{name}{suffix}"),
                                inner.width.saturating_sub(icon.width()) as usize,
                                if *is_dir { directory_style } else { text_style },
                            );
                            continue;
                        }
                    }

                    let suffix = if *is_dir { "/" } else { "" };
                    surface.set_stringn(
                        inner.x,
                        inner.y + i as u16,
                        format!("{name}{suffix}"),
                        inner.width as usize,
                        if *is_dir { directory_style } else { text_style },
                    );
                }
                return;
            }

            let alt_text = preview.placeholder();
            let x = inner.x + inner.width.saturating_sub(alt_text.len() as u16) / 2;
            let y = inner.y + inner.height / 2;
            surface.set_stringn(x, y, alt_text, inner.width as usize, text_style);
            return;
        }
    };

    let mut offset = ViewPosition::default();
    if let Some((start_line, end_line)) = range {
        let height = end_line - start_line;
        let text = doc.text().slice(..);
        let start = text.line_to_char(start_line);
        let middle = text.line_to_char(start_line + height / 2);
        if height < inner.height as usize {
            let text_fmt = doc.text_format(inner.width, None);
            let annotations = TextAnnotations::default();
            (offset.anchor, offset.vertical_offset) = char_idx_at_visual_offset(
                text,
                middle,
                -(inner.height as isize / 2),
                0,
                &text_fmt,
                &annotations,
            );
            if start < offset.anchor {
                offset.anchor = start;
                offset.vertical_offset = 0;
            }
        } else {
            offset.anchor = start;
        }
    }

    let loader = cx.editor.syn_loader.load();
    let config = cx.editor.config();

    let syntax_highlighter =
        EditorView::doc_syntax_highlighter(doc, offset.anchor, area.height, &loader);
    let mut overlay_highlights = Vec::new();
    if doc
        .language_config()
        .and_then(|config| config.rainbow_brackets)
        .unwrap_or(config.rainbow_brackets)
        && let Some(overlay) = EditorView::doc_rainbow_highlights(
            doc,
            offset.anchor,
            area.height,
            &cx.editor.theme,
            &loader,
        )
    {
        overlay_highlights.push(overlay);
    }

    EditorView::doc_diagnostics_highlights_into(doc, &cx.editor.theme, &mut overlay_highlights);

    let mut decorations = DecorationManager::default();

    if let Some((start, end)) = range {
        let style = cx
            .editor
            .theme
            .try_get("ui.highlight")
            .unwrap_or_else(|| cx.editor.theme.get("ui.selection"));
        let draw_highlight = move |renderer: &mut TextRenderer, pos: LinePos| {
            if (start..=end).contains(&pos.doc_line) {
                let area = Rect::new(
                    renderer.viewport.x,
                    pos.visual_line,
                    renderer.viewport.width,
                    1,
                );
                renderer.set_style(area, style)
            }
        };
        decorations.add_decoration(draw_highlight);
    }

    render_document(
        surface,
        inner,
        doc,
        offset,
        &TextAnnotations::default(),
        syntax_highlighter,
        overlay_highlights,
        &cx.editor.theme,
        decorations,
    );
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
            _redraw: self._redraw.clone(),
        }
    }
}

#[derive(Error, Debug)]
#[error("picker has been shut down")]
pub struct InjectorShutdown;

impl<T, D> Injector<T, D> {
    pub fn push(&self, item: T) -> Result<(), InjectorShutdown> {
        if self.version != self.picker_version.load(atomic::Ordering::Relaxed) {
            return Err(InjectorShutdown);
        }

        inject_nucleo_item(&self.dst, &self.columns, item, &self.editor_data);
        Ok(())
    }
}

type ColumnFormatFn<T, D> = for<'a> fn(&'a T, &'a D) -> Cell<'a>;

pub struct Column<T, D> {
    name: Arc<str>,
    format: ColumnFormatFn<T, D>,
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
            filter: false,
            hidden: true,
        }
    }

    pub fn without_filtering(mut self) -> Self {
        self.filter = false;
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
    columns: Arc<[Column<T, D>]>,
    primary_column: usize,
    editor_data: Arc<D>,
    version: Arc<AtomicUsize>,
    matcher: Nucleo<T>,

    /// Current height of the completions box
    completion_height: u16,

    cursor: u32,
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
    /// Given an item in the picker, return the file path and line number to display.
    file_fn: Option<FileCallback<T>>,
    /// Given an item in the picker, return an explicit quicklist entry.
    quicklist_fn: Option<QuicklistCallback<T>>,
    /// An event handler for syntax highlighting the currently previewed file.
    preview_highlight_handler: Sender<Arc<Path>>,
    dynamic_query_handler: Option<Sender<DynamicQueryChange>>,
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> Picker<T, D> {
    pub fn stream(
        columns: impl IntoIterator<Item = Column<T, D>>,
        editor_data: D,
    ) -> (Nucleo<T>, Injector<T, D>) {
        let columns: Arc<[_]> = columns.into_iter().collect();
        let matcher_columns = columns.iter().filter(|col| col.filter).count() as u32;
        assert!(matcher_columns > 0);
        let matcher = Nucleo::new(
            Config::DEFAULT,
            Arc::new(event::redraw_callback()),
            None,
            matcher_columns,
        );
        let streamer = Injector {
            dst: matcher.injector(),
            columns,
            editor_data: Arc::new(editor_data),
            version: 0,
            picker_version: Arc::new(AtomicUsize::new(0)),
            _redraw: event::RequestRedrawOnDrop::default(),
        };
        (matcher, streamer)
    }

    pub fn new<C, O, F>(
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

    pub(super) fn new_with_callback_result<C, O, F>(
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
            Arc::new(event::redraw_callback()),
            None,
            matcher_columns,
        );
        let injector = matcher.injector();
        for item in options {
            inject_nucleo_item(&injector, &columns, item, &editor_data);
        }
        Self::with(
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
            matcher,
            editor_data,
            version,
            cursor: 0,
            prompt,
            query,
            truncate_start: true,
            show_preview: true,
            callback_fn: Box::new(callback_fn),
            default_action: Action::Replace,
            completion_height: 0,
            widths,
            preview: FilePreview::default(),
            file_fn: None,
            quicklist_fn: None,
            preview_highlight_handler: PreviewHighlightHandler::<T, D>::default().spawn(),
            dynamic_query_handler: None,
        }
    }

    pub fn injector(&self) -> Injector<T, D> {
        Injector {
            dst: self.matcher.injector(),
            columns: self.columns.clone(),
            editor_data: self.editor_data.clone(),
            version: self.version.load(atomic::Ordering::Relaxed),
            picker_version: self.version.clone(),
            _redraw: event::RequestRedrawOnDrop::default(),
        }
    }

    pub fn truncate_start(mut self, truncate_start: bool) -> Self {
        self.truncate_start = truncate_start;
        self
    }

    pub fn with_preview(
        mut self,
        preview_fn: impl for<'a> Fn(&'a Editor, &'a T) -> Option<FileLocation<'a>> + 'static,
    ) -> Self {
        self.file_fn = Some(Box::new(preview_fn));
        // assumption: if we have a preview we are matching paths... If this is ever
        // not true this could be a separate builder function
        self.matcher.update_config(Config::DEFAULT.match_paths());
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
        self.cursor = cursor;
        self
    }

    pub fn with_dynamic_query(
        mut self,
        callback: DynQueryCallback<T, D>,
        debounce_ms: Option<u64>,
    ) -> Self {
        let handler = DynamicQueryHandler::new(callback, debounce_ms).spawn();
        let event = DynamicQueryChange {
            query: self.primary_query(),
            // Treat the initial query as a paste.
            is_paste: true,
        };
        event::send_blocking(&handler, event);
        self.dynamic_query_handler = Some(handler);
        self
    }

    pub fn with_default_action(mut self, action: Action) -> Self {
        self.default_action = action;
        self
    }

    /// Move the cursor by a number of lines, either down (`Forward`) or up (`Backward`)
    pub fn move_by(&mut self, amount: u32, direction: Direction) {
        let len = self.matcher.snapshot().matched_item_count();

        if len == 0 {
            // No results, can't move.
            return;
        }

        match direction {
            Direction::Forward => {
                self.cursor = self.cursor.saturating_add(amount) % len;
            }
            Direction::Backward => {
                self.cursor = self.cursor.saturating_add(len).saturating_sub(amount) % len;
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
        self.cursor = 0;
    }

    /// Move the cursor to the last entry
    pub fn to_end(&mut self) {
        self.cursor = self
            .matcher
            .snapshot()
            .matched_item_count()
            .saturating_sub(1);
    }

    pub fn selection(&self) -> Option<&T> {
        self.matcher
            .snapshot()
            .get_matched_item(self.cursor)
            .map(|item| item.data)
    }

    fn apply_callback_result(
        &mut self,
        result: PickerCallbackResult<T, D>,
        editor: &Editor,
    ) -> bool {
        match result {
            PickerCallbackResult::Close => true,
            PickerCallbackResult::KeepOpen => false,
            PickerCallbackResult::Replace {
                options,
                editor_data,
            } => {
                self.replace_options(options, editor_data, editor);
                false
            }
        }
    }

    fn replace_options(&mut self, options: Vec<T>, editor_data: D, editor: &Editor) {
        // Cancel existing injectors before replacing the matcher contents.
        self.version.fetch_add(1, atomic::Ordering::Relaxed);
        self.matcher.restart(true);
        self.editor_data = Arc::new(editor_data);

        self.cursor = 0;
        self.prompt.clear(editor);
        self.handle_prompt_change(false);
        self.widths = visible_column_widths(&self.columns);
        self.preview.clear();

        let injector = self.matcher.injector();
        for item in options {
            inject_nucleo_item(&injector, &self.columns, item, &self.editor_data);
        }
    }

    /// Collects the picker's current matched items into quicklist entries.
    fn quicklist_entries(&self, editor: &Editor) -> Vec<QuicklistEntry> {
        let snapshot = self.matcher.snapshot();
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
        if let EventResult::Consumed(_) = self.prompt.handle_event(event, cx) {
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
        self.cursor = 0;
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
            self.matcher.pattern.reparse(
                i,
                pattern,
                CaseMatching::Smart,
                Normalization::Smart,
                is_append,
            );
        }
        // If this is a dynamic picker, notify the query hook that the primary
        // query might have been updated.
        if let Some(handler) = &self.dynamic_query_handler {
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
        image_picker: Option<&ratatui_image::picker::Picker>,
        image_size: Size,
    ) -> Option<(Preview<'picker, 'editor>, Option<(usize, usize)>)> {
        let snapshot = self.matcher.snapshot();
        let location = snapshot
            .get_matched_item(self.cursor)
            .and_then(|current| (self.file_fn.as_ref()?)(editor, current.data));
        let Some(location) = location else {
            self.preview.cancel_image_load(None);
            return None;
        };
        self.preview.get::<T, D>(
            editor,
            location,
            Some(&self.preview_highlight_handler),
            image_picker,
            image_size,
        )
    }

    fn render_picker(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let status = self.matcher.tick(10);
        let snapshot = self.matcher.snapshot();
        if status.changed {
            self.cursor = self
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

        let block = panel::bordered(&cx.editor.theme);
        let inner = block.inner(area);
        block.render(area, surface);

        let [prompt_area, separator_area, inner] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);

        // -- Render the input bar:

        let count = format!(
            "{}{}/{}",
            if status.running || self.matcher.active_injectors() > 0 {
                "(running) "
            } else {
                ""
            },
            snapshot.matched_item_count(),
            snapshot.item_count(),
        );

        let prompt_area = prompt_area.clip_left(1);
        let [line_area, count_area, _right_padding] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(count.len() as u16),
            Constraint::Length(1),
        ])
        .areas(prompt_area);

        // render the prompt first since it will clear its background
        self.prompt.render(line_area, surface, cx);

        Paragraph::new(count)
            .style(text_style)
            .render(count_area, surface);

        let sep_style = cx.editor.theme.get("ui.background.separator");
        panel::top_border(sep_style).render(separator_area, surface);

        let rows = inner.height.saturating_sub(self.header_height()) as u32;
        let offset = self.cursor - (self.cursor % std::cmp::max(1, rows));
        let cursor = self.cursor.saturating_sub(offset);
        let end = offset
            .saturating_add(rows)
            .min(snapshot.matched_item_count());
        let mut indices = Vec::new();
        let mut matcher = MATCHER.lock();
        matcher.config = Config::DEFAULT;
        if self.file_fn.is_some() {
            matcher.config.set_match_paths()
        }

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
                let mut cell = column.format(item.data, &self.editor_data);
                let width = if column.filter {
                    snapshot.pattern().column_pattern(matcher_index).indices(
                        item.matcher_columns[matcher_index].slice(..),
                        &mut matcher,
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

        let block = panel::horizontally_padded(&cx.editor.theme);
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.is_empty() {
            self.preview.cancel_image_load(None);
            return;
        }
        let image_size = Size::new(inner.width, inner.height.saturating_sub(2).max(1));
        if let Some((preview, range)) = self.get_preview(cx.editor, cx.image_picker, image_size) {
            render_preview_content(preview, range, area, inner, surface, cx);
        }
    }
}

fn render_preview_placeholder(
    surface: &mut Surface,
    area: Rect,
    placeholder: &str,
    style: view::theme::Style,
) {
    let x = area.x + area.width.saturating_sub(placeholder.len() as u16) / 2;
    let y = area.y + area.height / 2;
    surface.set_stringn(x, y, placeholder, area.width as usize, style);
}

impl<I: 'static + Send + Sync, D: 'static + Send + Sync> Component for Picker<I, D> {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // +---------+ +---------+
        // |prompt   | |preview  |
        // +---------+ |         |
        // |picker   | |         |
        // |         | |         |
        // +---------+ +---------+

        let render_preview =
            self.show_preview && self.file_fn.is_some() && area.width > MIN_AREA_WIDTH_FOR_PREVIEW;

        let (picker_area, preview_area) = split_picker_area(area, render_preview);
        self.render_picker(picker_area, surface, cx);

        if let Some(preview_area) = preview_area {
            self.render_preview(preview_area, surface, cx);
        } else {
            self.preview.cancel_image_load(None);
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut Context) -> EventResult {
        // TODO: keybinds for scrolling preview

        let key_event = match event {
            Event::Key(event) => *event,
            Event::Paste(..) => return self.prompt_handle_event(event, ctx),
            Event::Resize(..) => return EventResult::Consumed(None),
            // Picker is a modal and should consume mouse events so clicks don't fall
            // through to the editor underneath
            Event::Mouse(_) => return EventResult::Consumed(None),
            _ => return EventResult::Ignored(None),
        };

        let close_fn = |picker: &mut Self| {
            picker.preview.cancel_image_load(None);
            // if the picker is very large don't store it as last_picker to avoid
            // excessive memory consumption
            let callback: compositor::Callback =
                if picker.matcher.snapshot().item_count() > 1_000_000 {
                    Box::new(|compositor: &mut Compositor, _ctx| {
                        // remove the layer
                        compositor.pop();
                    })
                } else {
                    // stop streaming in new items in the background, really we should
                    // be restarting the stream somehow once the picker gets
                    // reopened instead (like for an FS crawl) that would also remove the
                    // need for the special case above but that is pretty tricky
                    picker.version.fetch_add(1, atomic::Ordering::Relaxed);
                    Box::new(|compositor: &mut Compositor, _ctx| {
                        // remove the layer
                        compositor.last_picker = compositor.pop();
                    })
                };
            EventResult::Consumed(Some(callback))
        };

        match key_event {
            shift!(Tab) | key!(Up) | ctrl!('p') => {
                self.move_by(1, Direction::Backward);
            }
            key!(Tab) | key!(Down) | ctrl!('n') => {
                self.move_by(1, Direction::Forward);
            }
            key!(PageDown) | ctrl!('d') => {
                self.page_down();
            }
            key!(PageUp) | ctrl!('u') => {
                self.page_up();
            }
            key!(Home) => {
                self.to_start();
            }
            key!(End) => {
                self.to_end();
            }
            key!(Esc) | ctrl!('c') => return close_fn(self),
            ctrl!('q') => {
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
            alt!(Enter) => {
                if let Some(option) = self.selection() {
                    let result = (self.callback_fn)(ctx, option, self.default_action);
                    self.apply_callback_result(result, ctx.editor);
                }
            }
            key!(Enter) => {
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
                    if self.apply_callback_result(callback_result, ctx.editor) {
                        return close_fn(self);
                    }
                }
            }
            ctrl!('s') => {
                let callback_result = self
                    .selection()
                    .map_or(PickerCallbackResult::Close, |option| {
                        (self.callback_fn)(ctx, option, Action::HorizontalSplit)
                    });
                if self.apply_callback_result(callback_result, ctx.editor) {
                    return close_fn(self);
                }
            }
            ctrl!('v') => {
                let callback_result = self
                    .selection()
                    .map_or(PickerCallbackResult::Close, |option| {
                        (self.callback_fn)(ctx, option, Action::VerticalSplit)
                    });
                if self.apply_callback_result(callback_result, ctx.editor) {
                    return close_fn(self);
                }
            }
            ctrl!('t') => {
                self.toggle_preview();
            }
            _ => {
                self.prompt_handle_event(event, ctx);
            }
        }

        EventResult::Consumed(None)
    }

    fn cursor(&self, area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        let render_preview =
            self.show_preview && self.file_fn.is_some() && area.width > MIN_AREA_WIDTH_FOR_PREVIEW;
        let (picker_area, _) = split_picker_area(area, render_preview);
        let area = panel::bordered(&editor.theme)
            .inner(picker_area)
            .with_height(1)
            .clip_left(1);

        self.prompt.cursor(area, editor)
    }

    fn required_size(&mut self, (width, height): (u16, u16)) -> Option<(u16, u16)> {
        self.completion_height = height.saturating_sub(4 + self.header_height());
        Some((width, height))
    }

    fn id(&self) -> Option<&'static str> {
        Some(ID)
    }
}
impl<T: 'static + Send + Sync, D> Drop for Picker<T, D> {
    fn drop(&mut self) {
        // ensure we cancel any ongoing background threads streaming into the picker
        self.version.fetch_add(1, atomic::Ordering::Relaxed);
    }
}

pub(super) enum PickerCallbackResult<T, D> {
    Close,
    KeepOpen,
    Replace { options: Vec<T>, editor_data: D },
}

type PickerCallback<T, D> = Box<dyn Fn(&mut Context, &T, Action) -> PickerCallbackResult<T, D>>;

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
}
