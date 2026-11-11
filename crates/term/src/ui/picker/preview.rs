//! Picker preview cache, background loading, cancellation, and rendering.

use super::{
    scroll::{self, PreviewScroll},
    Picker,
};
use crate::{
    compositor::DrawContext,
    job,
    ui::{
        document::{render_document, LinePos, TextRenderer},
        highlights,
        image::{cached_image, invalidate_cached_image, load_image, render_image, PreparedImage},
        overlay::Overlay,
        text_decorations::DecorationManager,
    },
};
use ratatui_image::picker::Picker as ImagePicker;
use std::{
    borrow::Cow,
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use stdx::rope::RopeExt as _;
use text_layout::text_annotations::TextAnnotations;
use text_layout::{char_idx_at_visual_offset, doc_formatter::TextFormat, visual_offset_from_block};
use theme::icons::ICONS;
use tui::{buffer::Buffer as Surface, layout::Size};
use view::{
    features::workspace_changes::is_ignore_file, graphics::Rect, view::ViewPosition, Document,
    DocumentId, Editor,
};

/// Biggest file size to preview in bytes.
pub const MAX_FILE_SIZE_FOR_PREVIEW: u64 = 10 * 1024 * 1024;

#[cfg(test)]
#[path = "syntax_preview_tests.rs"]
mod syntax_preview_tests;

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

/// File path and range of lines (used to align and highlight lines)
pub type FileLocation<'a> = (PathOrId<'a>, Option<(usize, usize)>);

pub enum CachedPreview {
    Text(Box<view::document::PreviewText>),
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
    fn document(&self) -> Option<PreviewDocument<'_>> {
        match self {
            Preview::EditorDocument(doc) => Some(PreviewDocument::Live(doc)),
            Preview::Cached(CachedPreview::Text(doc)) => Some(PreviewDocument::Cached(doc)),
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
                CachedPreview::Text(_) => "<Invalid file location>",
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

#[derive(Clone, Copy)]
enum PreviewDocument<'a> {
    Live(&'a Document),
    Cached(&'a view::document::PreviewText),
}

impl<'a> PreviewDocument<'a> {
    fn text(self) -> &'a editor_core::Rope {
        match self {
            Self::Live(doc) => doc.text(),
            Self::Cached(text) => text.text(),
        }
    }
    fn text_format(self, width: u16, theme: &theme::Theme) -> TextFormat {
        match self {
            Self::Live(doc) => doc.text_format(width, Some(theme)),
            Self::Cached(text) => text.text_format(width, Some(theme)),
        }
    }
    fn language_config(self) -> Option<&'a language_config::LanguageConfiguration> {
        match self {
            Self::Live(doc) => doc.language_config(),
            Self::Cached(text) => text.language_config(),
        }
    }
    fn syntax(self) -> Option<&'a language::Syntax> {
        match self {
            Self::Live(doc) => doc.syntax(),
            Self::Cached(text) => text.syntax(),
        }
    }
    fn syntax_loader(self) -> &'a language::syntax::Loader {
        match self {
            Self::Live(doc) => doc.syntax_loader(),
            Self::Cached(text) => text.syntax_loader(),
        }
    }
    fn diagnostics(self) -> &'a [diagnostics::Diagnostic] {
        match self {
            Self::Live(doc) => doc.diagnostics(),
            Self::Cached(text) => text.diagnostics(),
        }
    }
}

#[derive(Default)]
pub(super) struct FileScroll {
    target: Option<(view::quicklist::QuicklistTarget, Option<(usize, usize)>)>,
    pub scroll: PreviewScroll,
    initialized: bool,
    metrics: Option<(editor_core::Rope, TextFormat, usize)>,
    position: Option<(usize, ViewPosition)>,
}

impl FileScroll {
    pub fn select(&mut self, (target, range): &FileLocation<'_>) {
        let target = match target {
            PathOrId::Path(path) => view::quicklist::QuicklistTarget::Path(path.to_path_buf()),
            PathOrId::Id(id) => view::quicklist::QuicklistTarget::Document(*id),
        };
        if self
            .target
            .as_ref()
            .is_none_or(|(old, old_range)| old != &target || old_range != range)
        {
            *self = Self {
                target: Some((target, *range)),
                ..Self::default()
            };
        }
    }

    pub(super) fn location(&self) -> Option<FileLocation<'_>> {
        let (target, range) = self.target.as_ref()?;
        let target = match target {
            view::quicklist::QuicklistTarget::Document(id) => PathOrId::Id(*id),
            view::quicklist::QuicklistTarget::Path(path) => PathOrId::Path(path),
        };
        Some((target, *range))
    }

    fn document_height(&mut self, text: &editor_core::Rope, format: &TextFormat) -> usize {
        if !format.soft_wrap {
            return text.len_lines(stdx::rope::LINE_TYPE);
        }
        // An unchanged rope clone gives us a constant-time text identity check.
        if let Some((_, _, height)) = self.metrics.as_ref().filter(|(cached, old, _)| {
            editor_core::ropey::extra::esoterica::ropes_are_instances(cached, text) && old == format
        }) {
            return *height;
        }
        let height = text_layout::softwrapped_dimensions(text.slice(..), format).0;
        self.metrics = Some((text.clone(), format.clone(), height));
        self.position = None;
        height
    }
}

pub(super) struct FilePreview {
    image_cache: Option<crate::ui::image::ImageCache>,
    preview_cache: HashMap<Arc<Path>, CacheEntry>,
    read_buffer: Vec<u8>,
    image_task: Option<ImagePreviewTask>,
    syntax_path: Option<Arc<Path>>,
    events: Option<tokio::sync::broadcast::Receiver<view::file_watcher::Events>>,
}

struct CacheEntry {
    canonical_path: PathBuf,
    stamp: Option<FileStamp>,
    preview: CachedPreview,
    checked: tokio::time::Instant,
    invalidated: bool,
}

impl CacheEntry {
    fn new(path: &Path, stamp: Option<FileStamp>, preview: CachedPreview) -> Self {
        Self {
            canonical_path: stdx::path::canonicalize_existing(path),
            stamp,
            preview,
            checked: tokio::time::Instant::now(),
            invalidated: false,
        }
    }
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    len: u64,
    is_dir: bool,
}

impl FileStamp {
    fn read(path: &Path) -> Option<Self> {
        let metadata = path.metadata().ok()?;
        Some(Self {
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            len: metadata.len(),
            is_dir: metadata.is_dir(),
        })
    }
}

impl Default for FilePreview {
    fn default() -> Self {
        Self {
            image_cache: None,
            preview_cache: HashMap::new(),
            read_buffer: Vec::with_capacity(1024),
            image_task: None,
            syntax_path: None,
            events: None,
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
            if matches!(self.preview_cache.get(&task.path).map(|entry| &entry.preview), Some(CachedPreview::Image(image))
                if matches!(image.as_ref(), ImagePreview::Loading { .. }))
            {
                self.preview_cache.remove(&task.path);
            }
        }
    }

    fn cancel_syntax_load(&mut self, path: Option<&Path>) {
        if self.syntax_path.as_deref() == path {
            return;
        }
        if let Some(path) = self.syntax_path.take()
            && let Some(CachedPreview::Text(doc)) = self
                .preview_cache
                .get_mut(&path)
                .map(|entry| &mut entry.preview)
        {
            doc.cancel_syntax_request();
        }
    }

    pub(super) fn cancel_loads(&mut self, location: Option<(&Path, Size)>) {
        self.cancel_image_load(location);
        self.cancel_syntax_load(location.map(|(path, _)| path));
    }

    fn highlight<T: 'static + Send + Sync, D: 'static + Send + Sync>(
        &mut self,
        path: Arc<Path>,
        jobs: &crate::job::Jobs,
    ) {
        let Some(CachedPreview::Text(doc)) = self
            .preview_cache
            .get_mut(&path)
            .map(|entry| &mut entry.preview)
        else {
            return;
        };
        if let Some(request) = doc.syntax_request() {
            self.syntax_path = Some(path.clone());
            highlight_preview::<T, D>(jobs, path, request);
        }
    }

    pub(super) fn clear(&mut self) {
        self.cancel_loads(None);
        self.preview_cache.clear();
        self.read_buffer.clear();
    }

    fn remove(&mut self, path: &Path) {
        let canonical_path = self
            .preview_cache
            .get(path)
            .map_or(path, |entry| entry.canonical_path.as_path());
        if let Some(cache) = &self.image_cache {
            invalidate_cached_image(cache, canonical_path);
        }
        if self.syntax_path.as_deref() == Some(path)
            || self
                .image_task
                .as_ref()
                .is_some_and(|task| task.path.as_ref() == path)
        {
            self.cancel_loads(None);
        }
        self.preview_cache.remove(path);
    }

    fn invalidate(&mut self, path: &Path) {
        if let Some(entry) = self.preview_cache.get_mut(path)
            && matches!(entry.preview, CachedPreview::Text(_))
        {
            entry.invalidated = true;
        } else {
            self.remove(path);
        }
    }

    fn refresh_document(&mut self, path: &Path, editor: &Editor, stamp: Option<FileStamp>) -> bool {
        let Some(entry) = self.preview_cache.get_mut(path) else {
            return false;
        };
        let CachedPreview::Text(doc) = &mut entry.preview else {
            return false;
        };
        if stamp
            .as_ref()
            .is_none_or(|stamp| stamp.is_dir || stamp.len > MAX_FILE_SIZE_FOR_PREVIEW)
        {
            return false;
        }
        let Ok(refreshed) = view::document::PreviewText::open(
            path,
            editor.config.clone(),
            editor.syn_loader.load_full(),
        ) else {
            return false;
        };
        if refreshed.is_binary() {
            return false;
        }
        doc.refresh(refreshed);
        doc.refresh_diagnostics(editor);
        entry.stamp = stamp;
        entry.canonical_path = stdx::path::canonicalize_existing(path);
        entry.checked = tokio::time::Instant::now();
        entry.invalidated = false;
        true
    }

    fn receive_changes(&mut self, editor: &Editor) {
        use tokio::sync::broadcast::error::TryRecvError;
        if self.events.is_none() {
            self.events = Some(editor.file_watcher.subscribe());
        }
        loop {
            match self.events.as_mut().unwrap().try_recv() {
                Ok(events) => {
                    let paths: Vec<_> = self
                        .preview_cache
                        .iter()
                        .filter(|(_, entry)| {
                            let path = &entry.canonical_path;
                            events.iter().any(|event| {
                                if event.ty == view::file_watcher::EventType::Tempfile {
                                    return false;
                                }
                                let changed = event.path.as_std_path();
                                path.starts_with(changed)
                                    || changed.parent() == Some(path.as_path())
                                    || is_ignore_file(changed)
                            })
                        })
                        .map(|(path, _)| path.clone())
                        .collect();
                    for path in paths {
                        self.invalidate(&path);
                    }
                }
                Err(TryRecvError::Lagged(_)) => {
                    for path in self.preview_cache.keys().cloned().collect::<Vec<_>>() {
                        self.invalidate(&path);
                    }
                }
                Err(TryRecvError::Closed) => {
                    for path in self.preview_cache.keys().cloned().collect::<Vec<_>>() {
                        self.invalidate(&path);
                    }
                    self.events = Some(editor.file_watcher.subscribe());
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    }

    pub(super) fn cached<'preview, 'editor>(
        &'preview mut self,
        editor: &'editor Editor,
        (target, range): FileLocation<'_>,
    ) -> Option<(Preview<'preview, 'editor>, Option<(usize, usize)>)> {
        match resolve_target(editor, target)? {
            PathOrId::Id(id) => Some((Preview::EditorDocument(editor.document(id)?), range)),
            PathOrId::Path(path) => self
                .preview_cache
                .get_mut(path)
                .map(|entry| (Preview::Cached(&mut entry.preview), range)),
        }
    }

    /// Prepare the selected file. Rendering reads only through `cached`.
    pub(super) fn prepare<T: 'static + Send + Sync, D: 'static + Send + Sync>(
        &mut self,
        editor: &Editor,
        target: PathOrId<'_>,
        jobs: &crate::job::Jobs,
        image_picker: Option<&ratatui_image::picker::Picker>,
        image_size: Size,
    ) {
        self.image_cache
            .get_or_insert_with(|| jobs.image_cache.clone());
        self.receive_changes(editor);
        let target = resolve_target(editor, target);
        self.cancel_loads(match target {
            Some(PathOrId::Path(path)) => Some((path, image_size)),
            _ => None,
        });
        let Some(PathOrId::Path(path)) = target else {
            return;
        };
        let stamp = FileStamp::read(path);
        let loader = editor.syn_loader.load();
        let poll = &editor.config().files.reload.poll;
        let writing = editor.file_is_writing(path);
        if writing && !self.preview_cache.contains_key(path) {
            return;
        }
        if !writing
            && self.preview_cache.get(path).is_some_and(|entry| {
                entry.invalidated
                    || entry.stamp != stamp
                    || matches!(&entry.preview, CachedPreview::Text(doc)
                    if !std::ptr::eq(doc.syntax_loader(), loader.as_ref()))
                    || (matches!(entry.preview, CachedPreview::Directory(_))
                        && poll.enable
                        && entry.checked.elapsed() >= Duration::from_millis(poll.interval.max(100)))
            })
            && !self.refresh_document(path, editor, FileStamp::read(path))
        {
            self.remove(path);
        }
        if matches!(self.preview_cache.get(path).map(|entry| &entry.preview), Some(CachedPreview::Image(image))
            if image.size() != image_size || matches!(image.as_ref(), ImagePreview::Ready { image, .. }
                if image_picker.is_some_and(|picker| !image.is_current(path, picker, image_size))))
        {
            self.remove(path);
        }
        if let Some((path, _)) = self.preview_cache.get_key_value(path) {
            let path = Arc::clone(path);
            self.highlight::<T, D>(path, jobs);
            return;
        }

        let path: Arc<Path> = path.into();
        if let Some(image) = image_picker
            .and_then(|picker| cached_image(&jobs.image_cache, &path, picker, image_size))
        {
            // Other images remain available in the shared, bounded cache.
            self.preview_cache
                .retain(|_, entry| !matches!(entry.preview, CachedPreview::Image(_)));
            self.preview_cache.insert(
                path.clone(),
                CacheEntry::new(
                    &path,
                    stamp,
                    CachedPreview::Image(Box::new(ImagePreview::Ready {
                        size: image_size,
                        image,
                    })),
                ),
            );
            return;
        }
        let preview = std::fs::metadata(&path)
            .and_then(|metadata| {
                if metadata.is_dir() {
                    let files =
                        crate::ui::directory_content(&path, &editor.config().file_explorer)?;
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
                    let doc = view::document::PreviewText::open(
                        &path,
                        editor.config.clone(),
                        editor.syn_loader.load_full(),
                    )
                    .or(Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Cannot open document",
                    )))?;
                    Ok(CachedPreview::Text(Box::new(doc)))
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
                .retain(|_, entry| !matches!(entry.preview, CachedPreview::Image(_)));
        }
        self.preview_cache
            .insert(path.clone(), CacheEntry::new(&path, stamp, preview));
        self.highlight::<T, D>(path.clone(), jobs);
        if let Some(request) = request {
            self.image_task = Some(spawn_image_preview::<T, D>(
                jobs.sender(),
                jobs.image_cache.clone(),
                path.clone(),
                image_picker.unwrap().clone(),
                image_size,
                request,
            ));
        }
    }
}

/// Open text buffers are authoritative; binary buffers use their file preview.
fn resolve_target<'a>(editor: &'a Editor, target: PathOrId<'a>) -> Option<PathOrId<'a>> {
    match target {
        PathOrId::Id(id) => {
            let doc = editor.document(id)?;
            if doc.is_binary() {
                doc.path().map(PathOrId::Path)
            } else {
                Some(PathOrId::Id(id))
            }
        }
        PathOrId::Path(path) => Some(
            open_text_document(editor, path)
                .map_or(PathOrId::Path(path), |doc| PathOrId::Id(doc.id())),
        ),
    }
}

fn open_text_document<'a>(editor: &'a Editor, path: &Path) -> Option<&'a Document> {
    editor
        .document_by_path(stdx::path::canonicalize(path))
        .or_else(|| {
            let resolved = stdx::path::canonicalize_existing(path);
            editor.documents().find(|doc| {
                doc.path()
                    .is_some_and(|path| stdx::path::canonicalize_existing(path) == resolved)
            })
        })
        .filter(|doc| !doc.is_binary())
}

pub(super) fn render_preview_content(
    mut preview: Preview<'_, '_>,
    range: Option<(usize, usize)>,
    state: &mut FileScroll,
    inner: Rect,
    surface: &mut Surface,
    cx: &DrawContext,
) {
    let text_style = cx.editor.theme.get("ui.text");
    let directory_style = cx.editor.theme.get("ui.text.directory");
    if let Some(image) = preview.image() {
        state.scroll.update(inner.height, 0);
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
            if range.is_none_or(|(start, end)| {
                start <= end && end <= doc.text().len_lines(stdx::rope::LINE_TYPE)
            }) =>
        {
            doc
        }
        _ => {
            if let Some(dir_content) = preview.dir_content() {
                state.scroll.update(inner.height, dir_content.len());
                let content = scroll::text_area(inner);
                for (i, (path, is_dir)) in dir_content
                    .iter()
                    .skip(state.scroll.offset)
                    .take(inner.height as usize)
                    .enumerate()
                {
                    let name = path
                        .file_name()
                        .map_or_else(|| Cow::Borrowed(".."), |name| name.to_string_lossy());

                    if cx.editor.config().icons {
                        let icon = if *is_dir {
                            ICONS.fs().directory().get_with_style_or_default(
                                &name,
                                path.file_name().is_none(),
                                &cx.editor.theme,
                                directory_style,
                            )
                        } else {
                            ICONS
                                .fs()
                                .file()
                                .get_with_style_or_default(path, &cx.editor.theme)
                        }
                        .with_no_left_padding();
                        surface.set_stringn(
                            inner.x,
                            inner.y + i as u16,
                            icon.glyph(),
                            content.width as usize,
                            icon.style(),
                        );
                        let suffix = if *is_dir { "/" } else { "" };
                        surface.set_stringn(
                            inner.x + icon.width(),
                            inner.y + i as u16,
                            format!("{name}{suffix}"),
                            content.width.saturating_sub(icon.width()) as usize,
                            if *is_dir { directory_style } else { text_style },
                        );
                        continue;
                    }

                    let suffix = if *is_dir { "/" } else { "" };
                    surface.set_stringn(
                        inner.x,
                        inner.y + i as u16,
                        format!("{name}{suffix}"),
                        content.width as usize,
                        if *is_dir { directory_style } else { text_style },
                    );
                }
                state.scroll.render(inner, surface, &cx.editor.theme);
                return;
            }

            state.scroll.update(inner.height, 0);
            let alt_text = preview.placeholder();
            let x = inner.x + inner.width.saturating_sub(alt_text.len() as u16) / 2;
            let y = inner.y + inner.height / 2;
            surface.set_stringn(x, y, alt_text, inner.width as usize, text_style);
            return;
        }
    };

    let scrollbar_area = inner;
    let inner = scroll::text_area(inner);
    if inner.is_empty() {
        return;
    }
    let format = doc.text_format(inner.width, &cx.editor.theme);
    let content_height = state.document_height(doc.text(), &format);
    let mut offset = ViewPosition::default();
    if !state.initialized
        && let Some((start_line, end_line)) = range
    {
        let last_line = doc
            .text()
            .len_lines(stdx::rope::LINE_TYPE)
            .saturating_sub(1);
        let start_line = start_line.min(last_line);
        let end_line = end_line.max(start_line).min(last_line);
        let height = end_line - start_line;
        let text = doc.text().slice(..);
        let start = text.line_to_char_idx(start_line);
        let middle = text.line_to_char_idx(start_line + height / 2);
        if height < inner.height as usize {
            let annotations = TextAnnotations::default();
            (offset.anchor, offset.vertical_offset) = char_idx_at_visual_offset(
                text,
                middle,
                -(inner.height as isize / 2),
                0,
                &format,
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

    if !state.initialized {
        state.scroll.offset = if format.soft_wrap {
            visual_offset_from_block(
                doc.text().slice(..),
                0,
                offset.anchor,
                &format,
                &TextAnnotations::default(),
            )
            .0
            .row + offset.vertical_offset
        } else {
            doc.text().char_to_line_idx(offset.anchor)
        };
        state.initialized = true;
    }
    state.scroll.update(inner.height, content_height);
    if !format.soft_wrap {
        offset.anchor = doc.text().line_to_char_idx(state.scroll.offset);
    } else if let Some((_, cached)) = state
        .position
        .filter(|(row, _)| *row == state.scroll.offset)
    {
        offset = cached;
    } else {
        let (anchor, delta) =
            state
                .position
                .map_or((0, state.scroll.offset as isize), |(row, position)| {
                    (
                        position.anchor,
                        (state.scroll.offset as isize)
                            .saturating_sub(row as isize)
                            .saturating_add(position.vertical_offset as isize),
                    )
                });
        (offset.anchor, offset.vertical_offset) = char_idx_at_visual_offset(
            doc.text().slice(..),
            anchor,
            delta,
            0,
            &format,
            &TextAnnotations::default(),
        );
        state.position = Some((state.scroll.offset, offset));
    }

    let config = cx.editor.config();

    let layout = view::view::ViewLayout {
        text: doc.text().slice(..),
        viewport: inner,
        offset,
        format,
        annotations: TextAnnotations::default(),
    };
    let mut overlay_highlights = Vec::new();
    if let PreviewDocument::Live(live) = doc
        && let Some(highlights) =
            crate::ui::change_review::document::highlights(live.into(), &layout, &cx.editor.theme)
    {
        overlay_highlights.push(highlights);
    }
    if doc
        .language_config()
        .and_then(|config| config.rainbow_brackets)
        .unwrap_or(config.rainbow_brackets.enable)
        && let Some(syntax) = doc.syntax()
        && let Some(overlay) = highlights::syntax_rainbow_highlights(
            syntax,
            doc.syntax_loader(),
            &layout,
            &cx.editor.theme,
        )
    {
        overlay_highlights.push(overlay);
    }

    highlights::diagnostic_styles(doc.diagnostics(), &cx.editor.theme, &mut overlay_highlights);

    let mut decorations = DecorationManager::default();
    if let PreviewDocument::Live(live) = doc
        && live.review().is_some()
    {
        decorations.add_decoration(crate::ui::change_review::document::line_backgrounds(
            live.into(),
            &cx.editor.theme,
        ));
    }

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

    match doc {
        PreviewDocument::Live(doc) => render_document(
            surface,
            doc.into(),
            &layout,
            overlay_highlights,
            &cx.editor.theme,
            decorations,
        ),
        PreviewDocument::Cached(text) => {
            let visible = layout.visible_byte_range();
            overlay_highlights.insert(
                0,
                text.conflict_marker_highlights(
                    layout.text.byte_to_char_idx(visible.start)
                        ..layout.text.byte_to_char_idx(visible.end),
                    &cx.editor.theme,
                ),
            );
            let tab_width = text.tab_width();
            let settings = crate::ui::document::RenderSettings {
                whitespace: &config.whitespace,
                indent_guides: &config.indent_guides,
                tab_width,
                indent_width: text.indent_style().indent_width(tab_width),
            };
            let mut renderer = TextRenderer::new(
                surface,
                settings,
                &cx.editor.theme,
                text_layout::DisplayPosition::new(
                    layout.offset.vertical_offset,
                    layout.offset.horizontal_offset,
                ),
                layout.viewport,
            );
            crate::ui::document::render_text(
                &mut renderer,
                layout.text,
                layout.offset.anchor,
                &layout.format,
                &layout.annotations,
                highlights::text_syntax_styles(
                    text.syntax(),
                    text.syntax_loader(),
                    &[],
                    &layout,
                    &cx.editor.theme,
                ),
                overlay_highlights,
                &cx.editor.theme,
                decorations,
            );
        }
    }
    state
        .scroll
        .render(scrollbar_area, surface, &cx.editor.theme);
}

fn render_preview_placeholder(
    surface: &mut Surface,
    area: Rect,
    placeholder: &str,
    style: theme::Style,
) {
    let x = area.x + area.width.saturating_sub(placeholder.len() as u16) / 2;
    let y = area.y + area.height / 2;
    surface.set_stringn(x, y, placeholder, area.width as usize, style);
}

const IMAGE_PREVIEW_DELAY: Duration = Duration::from_millis(150);

struct ImagePreviewTask {
    pub path: Arc<Path>,
    pub size: Size,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ImagePreviewTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn spawn_image_preview<T: 'static + Send + Sync, D: 'static + Send + Sync>(
    jobs: job::JobSender,
    cache: crate::ui::image::ImageCache,
    path: Arc<Path>,
    image_picker: ImagePicker,
    size: Size,
    request: Arc<()>,
) -> ImagePreviewTask {
    let handle_path = path.clone();
    let task_path = path.clone();
    let task = tokio::spawn(async move {
        // Do not decode or transmit images that the user only scrolls past.
        tokio::time::sleep(IMAGE_PREVIEW_DELAY).await;
        let result = load_image(cache, task_path, image_picker, size).await;

        jobs.send(move |compositor, _| {
            let Some(Overlay {
                content: picker, ..
            }) = compositor.find::<Overlay<Picker<T, D>>>()
            else {
                return;
            };
            let Some(entry) = picker.preview.preview_cache.get_mut(&path) else {
                return;
            };
            let preview = &mut entry.preview;
            // A previous selection of the same path must not replace its newer request.
            if !matches!(preview, CachedPreview::Image(image)
                if matches!(image.as_ref(), ImagePreview::Loading { request: current, .. }
                    if Arc::ptr_eq(current, &request)))
            {
                return;
            }
            *preview = match result {
                Ok(image) => CachedPreview::Image(Box::new(ImagePreview::Ready { size, image })),
                Err(err) => {
                    log::debug!(
                        "failed to decode image preview for {}: {err}",
                        path.display()
                    );
                    CachedPreview::Binary
                }
            };
        })
        .await;
    });
    ImagePreviewTask {
        path: handle_path,
        size,
        task,
    }
}

const SYNTAX_PREVIEW_DELAY: Duration = Duration::from_millis(50);

fn highlight_preview<T: 'static + Send + Sync, D: 'static + Send + Sync>(
    jobs: &job::Jobs,
    path: Arc<Path>,
    request: view::document::SyntaxRequest,
) {
    jobs.add(job::Job {
        future: Box::pin(async move {
            tokio::select! {
                biased;
                _ = request.canceled() => return Ok(None),
                _ = tokio::time::sleep(SYNTAX_PREVIEW_DELAY) => {},
            }
            let Some(result) = request.compute().await else {
                return Ok(None);
            };
            Ok(Some(job::Callback::ui(move |compositor, cx| {
                let editor = &mut *cx.editor;
                let Some(Overlay {
                    content: picker, ..
                }) = compositor.find::<Overlay<Picker<T, D>>>()
                else {
                    return;
                };
                let Some(CachedPreview::Text(doc)) = picker
                    .preview
                    .preview_cache
                    .get_mut(&path)
                    .map(|entry| &mut entry.preview)
                else {
                    return;
                };
                if doc.complete_syntax(&request, result) {
                    doc.refresh_diagnostics(editor);
                }
            })))
        }),
        wait: false,
    });
}

#[cfg(test)]
mod image_preview_tests {
    use std::fs;

    use ratatui_image::{picker::ProtocolType, protocol::Protocol, Image};
    use tui::{buffer::Buffer, layout::Rect, widgets::Widget};

    use super::*;
    use crate::ui::image::decode_image_preview;

    #[test]
    fn decodes_a_supported_image_into_the_selected_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        image::DynamicImage::new_rgba8(2, 2).save(&path).unwrap();

        let (protocol, details) =
            decode_image_preview(&path, &ImagePicker::halfblocks(), Size::new(20, 10)).unwrap();

        assert!(matches!(protocol, Protocol::Halfblocks(_)));
        assert_eq!(protocol.size(), Size::new(1, 1));
        assert!(details.starts_with("PNG · 2 × 2 px · "));
    }

    #[test]
    fn prepares_a_fitted_image_and_keeps_original_dimensions_in_caption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        image::DynamicImage::new_rgb8(800, 600).save(&path).unwrap();
        let (protocol, details) =
            decode_image_preview(&path, &ImagePicker::halfblocks(), Size::new(40, 10)).unwrap();
        assert!(protocol.size().width <= 40);
        assert!(protocol.size().height <= 10);
        assert!(details.starts_with("PNG · 800 × 600 px · "));
    }

    #[test]
    fn kitty_preview_transmits_pixels_only_on_its_first_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        image::DynamicImage::new_rgb8(20, 20).save(&path).unwrap();
        let mut picker = ImagePicker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        let (protocol, _) = decode_image_preview(&path, &picker, Size::new(20, 10)).unwrap();
        let area = Rect::new(0, 0, 20, 10);
        let mut buffer = Buffer::empty(area);
        Image::new(&protocol).render(area, &mut buffer);
        assert!(buffer
            .content
            .iter()
            .any(|cell| cell.symbol().contains("\x1b_G")));
        buffer.reset();
        Image::new(&protocol).render(area, &mut buffer);
        assert!(!buffer
            .content
            .iter()
            .any(|cell| cell.symbol().contains("\x1b_G")));
    }

    #[tokio::test]
    async fn moving_past_an_image_cancels_its_debounced_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        image::DynamicImage::new_rgb8(2, 2).save(&path).unwrap();
        let mut jobs = job::Jobs::new();
        let task = spawn_image_preview::<(), ()>(
            jobs.sender(),
            jobs.image_cache.clone(),
            path.into(),
            ImagePicker::halfblocks(),
            Size::new(20, 10),
            Arc::new(()),
        );
        // The pending selection must not enqueue a render during fast navigation.
        assert!(
            tokio::time::timeout(Duration::from_millis(30), jobs.callbacks.recv())
                .await
                .is_err()
        );
        drop(task);
        assert!(
            tokio::time::timeout(IMAGE_PREVIEW_DELAY * 2, jobs.callbacks.recv())
                .await
                .is_err()
        );
    }

    #[test]
    fn rejects_non_image_binary_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-an-image.bin");
        fs::write(&path, b"\0\x01\x02not an image").unwrap();

        assert!(
            decode_image_preview(&path, &ImagePicker::halfblocks(), Size::new(20, 10)).is_err()
        );
    }
}
