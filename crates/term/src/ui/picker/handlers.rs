use std::{
    path::Path,
    sync::{atomic, Arc},
    time::Duration,
};

use event::AsyncHook;
use tokio::time::Instant;

use crate::{job, ui::overlay::Overlay};

use ratatui_image::{
    picker::Picker as ImagePicker,
    protocol::StatefulProtocol,
    thread::{ResizeRequest, ThreadProtocol},
};

use super::{CachedPreview, DynQueryCallback, ImagePreview, Picker};

const MAX_IMAGE_PREVIEW_DIMENSION: u32 = 32_768;
const MAX_IMAGE_PREVIEW_ALLOCATION: u64 = 128 * 1024 * 1024;

pub(super) fn spawn_image_preview<T: 'static + Send + Sync, D: 'static + Send + Sync>(
    path: Arc<Path>,
    image_picker: ImagePicker,
) {
    tokio::task::spawn_blocking(move || {
        let protocol = decode_image_preview(&path, &image_picker);

        job::dispatch_blocking(move |_editor, compositor| {
            let Some(Overlay {
                content: picker, ..
            }) = compositor.find::<Overlay<Picker<T, D>>>()
            else {
                return;
            };
            let Some(preview) = picker.preview.preview_cache.get_mut(&path) else {
                return;
            };
            if !matches!(
                preview,
                CachedPreview::Image(image)
                    if matches!(image.as_ref(), ImagePreview::Loading)
            ) {
                return;
            }

            *preview = match protocol {
                Ok(protocol) => CachedPreview::Image(Box::new(ImagePreview::Ready(Box::new(
                    spawn_resize_worker::<T, D>(path, protocol),
                )))),
                Err(err) => {
                    log::debug!(
                        "failed to decode image preview for {}: {err}",
                        path.display()
                    );
                    CachedPreview::Binary
                }
            };
        });
    });
}

fn decode_image_preview(
    path: &Path,
    image_picker: &ImagePicker,
) -> Result<StatefulProtocol, String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|err| err.to_string())?
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_PREVIEW_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_PREVIEW_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_PREVIEW_ALLOCATION);
    reader.limits(limits);
    let image = reader.decode().map_err(|err| err.to_string())?;
    Ok(image_picker.new_resize_protocol(image))
}

fn spawn_resize_worker<T: 'static + Send + Sync, D: 'static + Send + Sync>(
    path: Arc<Path>,
    protocol: StatefulProtocol,
) -> ThreadProtocol {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ResizeRequest>();

    tokio::spawn(async move {
        while let Some(request) = rx.recv().await {
            let result = tokio::task::spawn_blocking(move || request.resize_encode()).await;
            match result {
                Ok(Ok(response)) => {
                    let path = path.clone();
                    job::dispatch_blocking(move |_editor, compositor| {
                        let Some(Overlay {
                            content: picker, ..
                        }) = compositor.find::<Overlay<Picker<T, D>>>()
                        else {
                            return;
                        };
                        let Some(CachedPreview::Image(image)) =
                            picker.preview.preview_cache.get_mut(&path)
                        else {
                            return;
                        };
                        let ImagePreview::Ready(protocol) = image.as_mut() else {
                            return;
                        };
                        protocol.update_resized_protocol(response);
                    });
                }
                Ok(Err(err)) => {
                    mark_image_preview_failed::<T, D>(path.clone(), err.to_string());
                    break;
                }
                Err(err) => {
                    mark_image_preview_failed::<T, D>(path.clone(), err.to_string());
                    break;
                }
            }
        }
    });

    ThreadProtocol::new(tx, Some(protocol))
}

fn mark_image_preview_failed<T: 'static + Send + Sync, D: 'static + Send + Sync>(
    path: Arc<Path>,
    error: String,
) {
    job::dispatch_blocking(move |_editor, compositor| {
        log::debug!(
            "failed to prepare image preview for {}: {error}",
            path.display()
        );
        let Some(Overlay {
            content: picker, ..
        }) = compositor.find::<Overlay<Picker<T, D>>>()
        else {
            return;
        };
        let Some(preview) = picker.preview.preview_cache.get_mut(&path) else {
            return;
        };
        if matches!(
            preview,
            CachedPreview::Image(image)
                if matches!(image.as_ref(), ImagePreview::Ready(_))
        ) {
            *preview = CachedPreview::Image(Box::new(ImagePreview::Failed));
        }
    });
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

    use ratatui_image::protocol::StatefulProtocolType;

    use super::*;

    #[test]
    fn decodes_a_supported_image_into_the_selected_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        image::DynamicImage::new_rgba8(2, 2).save(&path).unwrap();

        let protocol = decode_image_preview(&path, &ImagePicker::halfblocks()).unwrap();

        assert!(matches!(
            protocol.protocol_type(),
            StatefulProtocolType::Halfblocks(_)
        ));
    }

    #[test]
    fn rejects_non_image_binary_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-an-image.bin");
        fs::write(&path, b"\0\x01\x02not an image").unwrap();

        assert!(decode_image_preview(&path, &ImagePicker::halfblocks()).is_err());
    }
}
