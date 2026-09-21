use std::{
    path::Path,
    sync::{atomic, Arc},
    time::Duration,
};

use event::AsyncHook;
use tokio::time::Instant;

use crate::{job, ui::overlay::Overlay};

use crate::ui::image::load_image;
use ratatui_image::picker::Picker as ImagePicker;
use tui::layout::Size;

use super::{CachedPreview, DynQueryCallback, ImagePreview, Picker};

const IMAGE_PREVIEW_DELAY: Duration = Duration::from_millis(150);

pub(super) struct ImagePreviewTask {
    pub path: Arc<Path>,
    pub size: Size,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ImagePreviewTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) fn spawn_image_preview<T: 'static + Send + Sync, D: 'static + Send + Sync>(
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

        job::dispatch(move |_editor, compositor| {
            let Some(Overlay {
                content: picker, ..
            }) = compositor.find::<Overlay<Picker<T, D>>>()
            else {
                return;
            };
            let Some(preview) = picker.preview.preview_cache.get_mut(&path) else {
                return;
            };
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

pub(super) struct PreviewHighlightHandler<T: 'static + Send + Sync, D: 'static + Send + Sync> {
    trigger: Option<Arc<Path>>,
    phantom_data: std::marker::PhantomData<(T, D)>,
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> Default for PreviewHighlightHandler<T, D> {
    fn default() -> Self {
        Self {
            trigger: None,
            phantom_data: Default::default(),
        }
    }
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> AsyncHook
    for PreviewHighlightHandler<T, D>
{
    type Event = Arc<Path>;

    fn handle_event(
        &mut self,
        path: Self::Event,
        timeout: Option<tokio::time::Instant>,
    ) -> Option<tokio::time::Instant> {
        if self
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger == &path)
        {
            // If the path hasn't changed, don't reset the debounce
            timeout
        } else {
            self.trigger = Some(path);
            Some(Instant::now() + Duration::from_millis(150))
        }
    }

    fn finish_debounce(&mut self) {
        let Some(path) = self.trigger.take() else {
            return;
        };

        job::dispatch_blocking(move |editor, compositor| {
            let Some(Overlay {
                content: picker, ..
            }) = compositor.find::<Overlay<Picker<T, D>>>()
            else {
                return;
            };

            let Some(CachedPreview::Document(doc)) = picker.preview.preview_cache.get_mut(&path)
            else {
                return;
            };

            if doc.syntax().is_some() {
                return;
            }

            let Some(language) = doc.language_config().map(|config| config.language()) else {
                return;
            };

            let loader = editor.syn_loader.load();
            let text = doc.text().clone();

            tokio::task::spawn_blocking(move || {
                let syntax = match editor_core::Syntax::new(text.slice(..), language, &loader) {
                    Ok(syntax) => syntax,
                    Err(err) => {
                        log::info!("highlighting picker preview failed: {err}");
                        return;
                    }
                };

                job::dispatch_blocking(move |editor, compositor| {
                    let Some(Overlay {
                        content: picker, ..
                    }) = compositor.find::<Overlay<Picker<T, D>>>()
                    else {
                        log::info!("picker closed before syntax highlighting finished");
                        return;
                    };
                    let Some(CachedPreview::Document(doc)) =
                        picker.preview.preview_cache.get_mut(&path)
                    else {
                        return;
                    };
                    let diagnostics = view::Editor::doc_diagnostics(
                        &editor.language_servers,
                        &editor.diagnostics,
                        doc,
                    );
                    doc.replace_diagnostics(diagnostics, &[], None);
                    doc.syntax = Some(syntax);
                });
            });
        });
    }
}

pub(super) struct DynamicQueryChange {
    pub query: Arc<str>,
    pub is_paste: bool,
}

pub(super) struct DynamicQueryHandler<T: 'static + Send + Sync, D: 'static + Send + Sync> {
    callback: Arc<DynQueryCallback<T, D>>,
    // Duration used as a debounce.
    // Defaults to 100ms if not provided via `Picker::with_dynamic_query`. Callers may want to set
    // this higher if the dynamic query is expensive - for example global search.
    debounce: Duration,
    last_query: Arc<str>,
    query: Option<Arc<str>>,
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> DynamicQueryHandler<T, D> {
    pub(super) fn new(callback: DynQueryCallback<T, D>, duration_ms: Option<u64>) -> Self {
        Self {
            callback: Arc::new(callback),
            debounce: Duration::from_millis(duration_ms.unwrap_or(100)),
            last_query: "".into(),
            query: None,
        }
    }
}

impl<T: 'static + Send + Sync, D: 'static + Send + Sync> AsyncHook for DynamicQueryHandler<T, D> {
    type Event = DynamicQueryChange;

    fn handle_event(&mut self, change: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        let DynamicQueryChange { query, is_paste } = change;
        if query == self.last_query {
            // If the search query reverts to the last one we requested, no need to
            // make a new request.
            self.query = None;
            None
        } else {
            self.query = Some(query);
            if is_paste {
                self.finish_debounce();
                None
            } else {
                Some(Instant::now() + self.debounce)
            }
        }
    }

    fn finish_debounce(&mut self) {
        let Some(query) = self.query.take() else {
            return;
        };
        self.last_query = query.clone();
        let callback = self.callback.clone();

        job::dispatch_blocking(move |editor, compositor| {
            let Some(Overlay {
                content: picker, ..
            }) = compositor.find::<Overlay<Picker<T, D>>>()
            else {
                return;
            };
            // Increment the version number to cancel any ongoing requests.
            picker.version.fetch_add(1, atomic::Ordering::Relaxed);
            picker.matcher.restart(false);
            let injector = picker.injector();
            let get_options = (callback)(&query, editor, picker.editor_data.clone(), &injector);
            tokio::spawn(async move {
                if let Err(err) = get_options.await {
                    log::info!("Dynamic request failed: {err}");
                }
                // NOTE: the Drop implementation of Injector will request a redraw when the
                // injector falls out of scope here, clearing the "running" indicator.
            });
        })
    }
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
        jobs.set_current();
        let task = spawn_image_preview::<(), ()>(
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
