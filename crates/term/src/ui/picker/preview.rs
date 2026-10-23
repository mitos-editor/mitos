//! Picker preview cache, background loading, cancellation, and rendering.

use super::Picker;
use crate::{
    compositor::Context,
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
use text_layout::char_idx_at_visual_offset;
use text_layout::text_annotations::TextAnnotations;
use theme::icons::ICONS;
use tui::{buffer::Buffer as Surface, layout::Size};
use view::{
    features::workspace_changes::is_ignore_file, graphics::Rect, view::ViewPosition, Document,
    DocumentId, Editor,
};

/// Biggest file size to preview in bytes.
pub const MAX_FILE_SIZE_FOR_PREVIEW: u64 = 10 * 1024 * 1024;

#[cfg(all(test, feature = "integration"))]
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

pub(super) struct FilePreview {
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
}

impl CacheEntry {
    fn new(path: &Path, stamp: Option<FileStamp>, preview: CachedPreview) -> Self {
        Self {
            canonical_path: stdx::path::canonicalize_existing(path),
            stamp,
            preview,
            checked: tokio::time::Instant::now(),
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
            && let Some(CachedPreview::Document(doc)) = self
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
        let Some(CachedPreview::Document(doc)) = self
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
        invalidate_cached_image(canonical_path);
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
                        self.remove(&path);
                    }
                }
                Err(TryRecvError::Lagged(_)) => {
                    for entry in self.preview_cache.values() {
                        invalidate_cached_image(&entry.canonical_path);
                    }
                    self.clear();
                }
                Err(TryRecvError::Closed) => {
                    self.clear();
                    self.events = Some(editor.file_watcher.subscribe());
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    }

    pub(super) fn get<'preview, 'editor, T: 'static + Send + Sync, D: 'static + Send + Sync>(
        &'preview mut self,
        editor: &'editor Editor,
        (path_or_id, range): FileLocation<'_>,
        jobs: &crate::job::Jobs,
        image_picker: Option<&ratatui_image::picker::Picker>,
        image_size: Size,
    ) -> Option<(Preview<'preview, 'editor>, Option<(usize, usize)>)> {
        self.receive_changes(editor);
        let path_or_id = match path_or_id {
            PathOrId::Id(id) => {
                let Some(doc) = editor.documents.get(&id) else {
                    self.cancel_loads(None);
                    return None;
                };
                if doc.is_binary() {
                    doc.path().map_or(PathOrId::Id(id), PathOrId::Path)
                } else {
                    PathOrId::Id(id)
                }
            }
            path => path,
        };
        self.cancel_loads(match path_or_id {
            PathOrId::Path(path) => Some((path, image_size)),
            PathOrId::Id(_) => None,
        });
        match path_or_id {
            PathOrId::Path(path) => {
                if let Some(doc) = editor
                    .document_by_path(stdx::path::canonicalize(path))
                    .filter(|doc| !doc.is_binary())
                {
                    self.cancel_syntax_load(None);
                    return Some((Preview::EditorDocument(doc), range));
                }

                let stamp = FileStamp::read(path);
                let loader = editor.syn_loader.load();
                let poll = &editor.config().files.reload.poll;
                if self.preview_cache.get(path).is_some_and(|entry| {
                    entry.stamp != stamp
                        || matches!(&entry.preview, CachedPreview::Document(doc)
                            if !std::ptr::eq(doc.syntax_loader(), loader.as_ref()))
                        || (matches!(entry.preview, CachedPreview::Directory(_))
                            && poll.enable
                            && entry.checked.elapsed()
                                >= Duration::from_millis(poll.interval.max(100)))
                }) {
                    self.remove(path);
                }
                if matches!(self.preview_cache.get(path).map(|entry| &entry.preview), Some(CachedPreview::Image(image))
                    if image.size() != image_size || matches!(image.as_ref(), ImagePreview::Ready { image, .. }
                        if image_picker.is_some_and(|picker| !image.is_current(path, picker, image_size))))
                {
                    self.remove(path);
                }
                if self.preview_cache.contains_key(path) {
                    let (path, _) = self.preview_cache.get_key_value(path).unwrap();
                    let path = Arc::clone(path);
                    self.highlight::<T, D>(path.clone(), jobs);
                    let preview = self.preview_cache.get_mut(&path).unwrap();
                    return Some((Preview::Cached(&mut preview.preview), range));
                }

                let path: Arc<Path> = path.into();
                if let Some(image) =
                    image_picker.and_then(|picker| cached_image(&path, picker, image_size))
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
                    return Some((
                        Preview::Cached(&mut self.preview_cache.get_mut(&path).unwrap().preview),
                        range,
                    ));
                }
                let preview = std::fs::metadata(&path)
                    .and_then(|metadata| {
                        if metadata.is_dir() {
                            let files = crate::ui::directory_content(
                                &path,
                                &editor.config().file_explorer,
                            )?;
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
                            let doc = Document::open_for_preview(
                                &path,
                                editor.config.clone(),
                                editor.syn_loader.load_full(),
                            )
                            .or(Err(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "Cannot open document",
                            )))?;
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
                        .retain(|_, entry| !matches!(entry.preview, CachedPreview::Image(_)));
                }
                self.preview_cache
                    .insert(path.clone(), CacheEntry::new(&path, stamp, preview));
                self.highlight::<T, D>(path.clone(), jobs);
                if let Some(request) = request {
                    self.image_task = Some(spawn_image_preview::<T, D>(
                        jobs.sender(),
                        path.clone(),
                        image_picker.unwrap().clone(),
                        image_size,
                        request,
                    ));
                }
                Some((
                    Preview::Cached(&mut self.preview_cache.get_mut(&path).unwrap().preview),
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

pub(super) fn render_preview_content(
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
            if range.is_none_or(|(start, end)| {
                start <= end && end <= doc.text().len_lines(stdx::rope::LINE_TYPE)
            }) =>
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

    let config = cx.editor.config();

    let syntax_highlighter = highlights::syntax_highlighter(doc, offset.anchor, area.height);
    let mut overlay_highlights = Vec::new();
    if doc
        .language_config()
        .and_then(|config| config.rainbow_brackets)
        .unwrap_or(config.rainbow_brackets.enable)
        && let Some(overlay) =
            highlights::rainbow_highlights(doc, offset.anchor, area.height, &cx.editor.theme)
    {
        overlay_highlights.push(overlay);
    }

    highlights::diagnostic_highlights(doc, &cx.editor.theme, &mut overlay_highlights);

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
        doc,
        &view::view::ViewLayout {
            text: doc.text().slice(..),
            viewport: inner,
            offset,
            format: doc.text_format(inner.width, Some(&cx.editor.theme)),
            annotations: TextAnnotations::default(),
        },
        syntax_highlighter,
        overlay_highlights,
        &cx.editor.theme,
        decorations,
    );
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
        let result = load_image(task_path, image_picker, size).await;

        jobs.send(move |_editor, compositor| {
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
            Ok(Some(job::Callback::EditorCompositor(Box::new(
                move |editor, compositor| {
                    let Some(Overlay {
                        content: picker, ..
                    }) = compositor.find::<Overlay<Picker<T, D>>>()
                    else {
                        return;
                    };
                    let Some(CachedPreview::Document(doc)) = picker
                        .preview
                        .preview_cache
                        .get_mut(&path)
                        .map(|entry| &mut entry.preview)
                    else {
                        return;
                    };
                    if request.complete(doc, result) {
                        let diagnostics = editor.diagnostics_for_document(doc);
                        doc.replace_diagnostics(diagnostics, &[], None);
                    }
                },
            ))))
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
