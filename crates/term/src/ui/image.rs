use ratatui_image::{
    picker::{Picker as ImagePicker, ProtocolType},
    protocol::Protocol,
    Image, Resize,
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tokio::{sync::oneshot, task::JoinHandle};
use tui::{
    buffer::Buffer,
    layout::{Alignment, Rect, Size},
    widgets::{Paragraph, Widget},
};
use view::{graphics::Style, Document, DocumentId};

const MAX_IMAGE_PREVIEW_DIMENSION: u32 = 32_768;
const MAX_IMAGE_PREVIEW_ALLOCATION: u64 = 128 * 1024 * 1024;
// A running decoder cannot be aborted. Keep newer requests cancelable while
// waiting instead of accumulating CPU-heavy workers.
static IMAGE_PREVIEW_WORKER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

const IMAGE_CACHE_CAPACITY: usize = 16;

event::runtime_local! {
    static IMAGE_CACHE: Mutex<ImageCache> = Mutex::new(ImageCache { entries: VecDeque::new() });
}

#[derive(Clone, PartialEq)]
struct ImageKey {
    path: PathBuf,
    modified: SystemTime,
    file_size: u64,
    size: Size,
    font_size: (u16, u16),
    protocol: ProtocolType,
}

impl ImageKey {
    fn new(path: &Path, picker: &ImagePicker, size: Size) -> Option<Self> {
        let path = stdx::path::canonicalize(path);
        let metadata = std::fs::metadata(&path).ok()?;
        Some(Self {
            path,
            modified: metadata.modified().ok()?,
            file_size: metadata.len(),
            size,
            font_size: (picker.font_size().width, picker.font_size().height),
            protocol: picker.protocol_type(),
        })
    }
}

pub struct PreparedImage {
    key: ImageKey,
    pub(super) protocol: Protocol,
    pub(super) details: String,
}

impl PreparedImage {
    pub(super) fn is_current(&self, path: &Path, picker: &ImagePicker, size: Size) -> bool {
        ImageKey::new(path, picker, size).as_ref() == Some(&self.key)
    }
}

struct ImageCache {
    // Least recently used first. Only encoded previews are retained, never source pixels.
    entries: VecDeque<Arc<PreparedImage>>,
}

impl ImageCache {
    fn get(&mut self, key: &ImageKey) -> Option<Arc<PreparedImage>> {
        let index = self.entries.iter().position(|image| image.key == *key)?;
        let image = self.entries.remove(index).unwrap();
        self.entries.push_back(image.clone());
        Some(image)
    }

    fn insert(&mut self, image: Arc<PreparedImage>) {
        self.entries.retain(|entry| {
            entry.key != image.key
                && (entry.key.path != image.key.path
                    || (entry.key.modified == image.key.modified
                        && entry.key.file_size == image.key.file_size))
        });
        self.entries.push_back(image);
        while self.entries.len() > IMAGE_CACHE_CAPACITY {
            self.entries.pop_front();
        }
    }
}

pub(super) fn cached_image(
    path: &Path,
    picker: &ImagePicker,
    size: Size,
) -> Option<Arc<PreparedImage>> {
    let key = ImageKey::new(path, picker, size)?;
    IMAGE_CACHE.lock().unwrap().get(&key)
}

type ImageResult = Result<Arc<PreparedImage>, String>;

pub(super) async fn load_image(path: Arc<Path>, picker: ImagePicker, size: Size) -> ImageResult {
    let permit = IMAGE_PREVIEW_WORKER.acquire().await.unwrap();
    let cache: &'static Mutex<ImageCache> = &IMAGE_CACHE;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let key = ImageKey::new(&path, &picker, size).ok_or("Image file is unavailable")?;
        // Another request may have completed while we waited for the worker.
        if let Some(image) = cache.lock().unwrap().get(&key) {
            return Ok(image);
        }
        let (protocol, details) = decode_image_preview(&path, &picker, size)?;
        let image = Arc::new(PreparedImage {
            key,
            protocol,
            details,
        });
        if image.is_current(&path, &picker, size) {
            cache.lock().unwrap().insert(image.clone());
        }
        Ok(image)
    })
    .await
    .map_err(|err| err.to_string())?
}

pub(super) fn decode_image_preview(
    path: &Path,
    image_picker: &ImagePicker,
    size: Size,
) -> Result<(Protocol, String), String> {
    let file_size = std::fs::metadata(path)
        .map_err(|err| err.to_string())?
        .len();
    let mut reader = image::ImageReader::open(path)
        .map_err(|err| err.to_string())?
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    let format = reader.format().ok_or("Unknown image format")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_PREVIEW_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_PREVIEW_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_PREVIEW_ALLOCATION);
    reader.limits(limits);
    let image = reader.decode().map_err(|err| err.to_string())?;
    let file_size = if file_size < 1024 {
        format!("{file_size} B")
    } else if file_size < 1024 * 1024 {
        format!("{:.1} KiB", file_size as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", file_size as f64 / (1024.0 * 1024.0))
    };
    let format = format!("{format:?}").to_uppercase();
    let details = format!(
        "{format} · {} × {} px · {file_size}",
        image.width(),
        image.height()
    );
    // Decode, resize, encode, and release the original pixels on the worker thread.
    // The UI only receives the terminal-sized, encoded image.
    let protocol = image_picker
        .new_protocol(image, size, Resize::Fit(None))
        .map_err(|err| err.to_string())?;
    Ok((protocol, details))
}

pub(super) fn render_image(
    protocol: &Protocol,
    details: &str,
    area: Rect,
    surface: &mut Buffer,
    style: Style,
) {
    let (image_area, caption_area) = image_preview_layout(area, protocol.size());
    Image::new(protocol).render(image_area, surface);
    Paragraph::new(details)
        .alignment(Alignment::Center)
        .style(style)
        .render(caption_area, surface);
}

pub(super) fn render_placeholder(text: &str, area: Rect, surface: &mut Buffer, style: Style) {
    let line = Rect::new(area.x, area.y + area.height / 2, area.width, 1).intersection(area);
    Paragraph::new(text)
        .alignment(Alignment::Center)
        .style(style)
        .render(line, surface);
}

pub(super) struct ImageDocumentView {
    doc: DocumentId,
    path: PathBuf,
    modified: SystemTime,
    size: Size,
    result: Option<ImageResult>,
    receiver: oneshot::Receiver<ImageResult>,
    task: Option<JoinHandle<()>>,
}

impl ImageDocumentView {
    pub fn new(doc: &Document, picker: ImagePicker, size: Size) -> Self {
        let path = doc.path().expect("binary documents have a path").to_owned();
        let result = cached_image(&path, &picker, size).map(Ok);
        let (sender, receiver) = oneshot::channel();
        let task = if result.is_some() {
            None
        } else {
            let worker_path = Arc::from(path.as_path());
            let redraw = event::redraw_callback();
            Some(tokio::spawn(async move {
                let result = load_image(worker_path, picker, size).await;
                if sender.send(result).is_ok() {
                    redraw();
                }
            }))
        };
        Self {
            doc: doc.id(),
            path,
            modified: doc.last_saved_time(),
            size,
            result,
            receiver,
            task,
        }
    }

    pub fn matches(&self, doc: &Document, size: Size) -> bool {
        self.doc == doc.id()
            && doc.path() == Some(self.path.as_path())
            && self.modified == doc.last_saved_time()
            && self.size == size
    }

    pub fn render(&mut self, area: Rect, surface: &mut Buffer, style: Style) {
        if self.result.is_none() {
            match self.receiver.try_recv() {
                Ok(result) => self.result = Some(result),
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.result = Some(Err("Image worker stopped".into()))
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        match &self.result {
            Some(Ok(image)) => render_image(&image.protocol, &image.details, area, surface, style),
            Some(Err(_)) => render_placeholder(
                "<Binary file — image preview unavailable>",
                area,
                surface,
                style,
            ),
            None => render_placeholder("<Loading image>", area, surface, style),
        }
    }
}

impl Drop for ImageDocumentView {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
pub(super) fn image_preview_layout(area: Rect, size: Size) -> (Rect, Rect) {
    let width = size.width.min(area.width);
    let height = size.height.min(area.height.saturating_sub(2));
    let top = area.y + area.height.saturating_sub(height + 2) / 2;
    let image = Rect::new(area.x + (area.width - width) / 2, top, width, height);
    let caption = Rect::new(area.x, top + height + 1, area.width, 1).intersection(area);
    (image, caption)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::ArcSwap;
    use editor_core::{syntax, Rope, Selection, Transaction};
    use std::fs;
    use view::{View, ViewId};

    fn open(path: &Path) -> Document {
        Document::open(
            path,
            None,
            true,
            Arc::new(ArcSwap::from_pointee(view::editor::Config::default())),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn prepared_images_are_shared_and_reopened_without_a_loading_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cached.png");
        image::DynamicImage::new_rgb8(40, 20).save(&path).unwrap();
        let picker = ImagePicker::halfblocks();
        let size = Size::new(60, 18);
        let prepared = load_image(Arc::from(path.as_path()), picker.clone(), size)
            .await
            .unwrap();
        let cached = cached_image(&path, &picker, size).unwrap();
        assert!(Arc::ptr_eq(&prepared, &cached));

        let doc = open(&path);
        for _ in 0..2 {
            let mut view = ImageDocumentView::new(&doc, picker.clone(), size);
            assert!(
                view.task.is_none(),
                "a cache hit must not start a loading task"
            );
            assert!(Arc::ptr_eq(
                view.result.as_ref().unwrap().as_ref().unwrap(),
                &prepared
            ));
            let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 20));
            view.render(buffer.area, &mut buffer, Style::default());
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("PNG · 40 × 20 px"));
            assert!(!text.contains("Loading"));
        }
    }

    #[tokio::test]
    async fn cached_images_are_invalidated_by_file_changes_and_render_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cached.png");
        image::DynamicImage::new_rgb8(40, 20).save(&path).unwrap();
        let picker = ImagePicker::halfblocks();
        let size = Size::new(60, 18);
        let original = load_image(Arc::from(path.as_path()), picker.clone(), size)
            .await
            .unwrap();
        assert!(cached_image(&path, &picker, Size::new(30, 18)).is_none());
        let mut other_protocol = picker.clone();
        other_protocol.set_protocol_type(ProtocolType::Kitty);
        assert!(cached_image(&path, &other_protocol, size).is_none());

        // A same-size rewrite must not reuse stale pixels.
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10))
            .unwrap();
        assert!(cached_image(&path, &picker, size).is_none());
        assert!(!original.is_current(&path, &picker, size));
        let updated = load_image(Arc::from(path.as_path()), picker.clone(), size)
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&original, &updated));
        assert!(Arc::ptr_eq(
            &updated,
            &cached_image(&path, &picker, size).unwrap()
        ));
    }

    #[test]
    fn image_cache_evicts_the_least_recently_used_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.png");
        image::DynamicImage::new_rgb8(2, 2).save(&path).unwrap();
        let picker = ImagePicker::halfblocks();
        let size = Size::new(20, 10);
        let key = ImageKey::new(&path, &picker, size).unwrap();
        let (protocol, details) = decode_image_preview(&path, &picker, size).unwrap();
        let entry = |index: usize| {
            Arc::new(PreparedImage {
                key: ImageKey {
                    path: PathBuf::from(index.to_string()),
                    ..key.clone()
                },
                protocol: protocol.clone(),
                details: details.clone(),
            })
        };
        let mut cache = ImageCache {
            entries: VecDeque::new(),
        };
        for index in 0..IMAGE_CACHE_CAPACITY {
            cache.insert(entry(index));
        }
        let oldest = entry(0);
        assert!(cache.get(&oldest.key).is_some());
        cache.insert(entry(IMAGE_CACHE_CAPACITY));
        assert_eq!(cache.entries.len(), IMAGE_CACHE_CAPACITY);
        assert!(cache.get(&oldest.key).is_some());
        assert!(cache.get(&entry(1).key).is_none());
    }

    #[tokio::test]
    async fn opening_an_image_never_decodes_its_bytes_as_editable_text() {
        let dir = tempfile::tempdir().unwrap();
        // Content detection also works when the filename has no image extension.
        let path = dir.path().join("image.bin");
        image::DynamicImage::new_rgb8(20, 20)
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let original = fs::read(&path).unwrap();
        let mut doc = open(&path);
        assert!(doc.is_binary());
        assert!(doc.readonly);
        assert_eq!(doc.text(), &Rope::new());
        assert!(doc.language_config().is_none());
        let insert = Transaction::insert(doc.text(), &Selection::point(0), "corrupt".into());
        assert!(!doc.apply(&insert, ViewId::default()));
        assert!(!doc.is_modified());
        assert!(doc.save::<PathBuf>(None, false).is_err());
        assert!(doc.save::<PathBuf>(None, true).is_err());
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[tokio::test]
    async fn opened_image_renders_with_metadata_and_invalidates_after_a_resize() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        image::DynamicImage::new_rgb8(40, 20).save(&path).unwrap();
        let doc = open(&path);
        let area = Rect::new(0, 0, 60, 20);
        let size = Size::new(60, 18);
        let mut image = ImageDocumentView::new(&doc, ImagePicker::halfblocks(), size);
        image.task.as_mut().unwrap().await.unwrap();
        let mut buffer = Buffer::empty(area);
        image.render(area, &mut buffer, Style::default());
        assert!(matches!(image.result, Some(Ok(_))));
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("PNG · 40 × 20 px"));
        assert!(image.matches(&doc, size));
        assert!(!image.matches(&doc, Size::new(30, 10)));
    }

    #[tokio::test]
    async fn unsupported_binary_files_render_a_disclaimer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.png");
        fs::write(&path, b"\x89PNG\r\n\0not an image\x1b[2J").unwrap();
        let doc = open(&path);
        let area = Rect::new(0, 0, 60, 20);
        let mut image = ImageDocumentView::new(&doc, ImagePicker::halfblocks(), Size::new(60, 18));
        image.task.as_mut().unwrap().await.unwrap();
        let mut buffer = Buffer::empty(area);
        image.render(area, &mut buffer, Style::default());
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("<Binary file — image preview unavailable>"));
        assert!(!text.contains('\x1b'));
        assert!(doc.text().len_bytes() == 0);
    }

    #[tokio::test]
    async fn reloading_a_binary_file_keeps_the_original_bytes_out_of_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        image::DynamicImage::new_rgb8(20, 20).save(&path).unwrap();
        let mut doc = open(&path);
        let mut view = View::new(doc.id(), Default::default());
        doc.set_selection(view.id, Selection::point(0));
        let providers = vcs::DiffProviderRegistry::default();
        doc.reload(&mut view, &providers, false).unwrap();
        assert!(doc.is_binary());
        assert!(doc.readonly);
        assert_eq!(doc.text().len_bytes(), 0);
        // Replacing it with text restores the normal editor path.
        fs::write(&path, "plain text\n").unwrap();
        doc.reload(&mut view, &providers, false).unwrap();
        assert!(!doc.is_binary());
        assert!(!doc.readonly);
        assert_eq!(doc.text().to_string(), "plain text\n");
        image::DynamicImage::new_rgb8(20, 20).save(&path).unwrap();
        doc.set_encoding("utf-16le").unwrap();
        doc.reload(&mut view, &providers, false).unwrap();
        assert!(doc.is_binary());
        assert!(doc.readonly);
        assert_eq!(doc.text().len_bytes(), 0);
    }

    #[test]
    fn unicode_text_with_a_bom_still_opens_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf16.txt");
        fs::write(&path, b"\xff\xfeh\0i\0\n\0").unwrap();
        let doc = open(&path);
        assert!(!doc.is_binary());
        assert_eq!(doc.text().to_string(), "hi\n");
    }
}
