//! One lazy syntax-event prefix per live pane. Geometry and overlays stay uncached.
use buffer::Snapshot;
use language::syntax::Loader;
use std::ops::Range;
use theme::Style;
use view::document::PresentedDocument;
#[cfg(test)]
use view::Document;
use view::{view::ViewLayout, DocumentId};

type Event = (usize, Style);
// Large ranges can arise from wrapping or a large terminal. Bypass retention
// for that key after this bound; ordinary rendering remains lazy and complete.
const MAX_EVENTS: usize = 65_536;

#[derive(PartialEq, Eq)]
struct Key {
    document: DocumentId,
    syntax: Option<u64>,
    semantic: u64,
    loader: u64,
    theme: u64,
    ranges: Vec<Range<usize>>,
}
struct Entry {
    key: Key,
    // Buffer identity matters as well as its version: presentation documents
    // can have the same document id and numeric version as another buffer.
    snapshot: Snapshot,
    prefix: Prefix,
}
#[derive(Default)]
pub(super) struct ViewportSyntaxCache {
    entry: Option<Entry>,
}

impl ViewportSyntaxCache {
    pub(super) fn styles<'a, I, F>(
        &'a mut self,
        doc: PresentedDocument<'a>,
        layout: &ViewLayout,
        theme: u64,
        fresh: F,
    ) -> impl Iterator<Item = Event> + 'a
    where
        I: Iterator<Item = Event> + 'a,
        F: FnOnce() -> I + 'a,
    {
        let key = Key {
            document: doc.source().id(),
            syntax: doc.syntax().map(|s| s.revision()),
            semantic: doc.semantic_tokens_revision(),
            loader: doc.syntax_loader().highlight_revision(),
            theme,
            ranges: layout.visible_byte_ranges(),
        };
        if self.entry.as_ref().is_none_or(|entry| {
            entry.key != key
                || !doc.is_snapshot_current(&entry.snapshot)
                || entry.prefix.invalidated
        }) {
            self.entry = Some(Entry {
                key,
                snapshot: doc.snapshot(),
                prefix: Prefix::default(),
            });
        }
        let entry = self.entry.as_mut().unwrap();
        Styles {
            prefix: &mut entry.prefix,
            loader: Some((doc.syntax_loader(), entry.key.loader)),
            index: 0,
            fresh: Some(fresh),
            input: None,
        }
    }
}

#[derive(Default)]
struct Prefix {
    events: Vec<Event>,
    exhausted: bool,
    bypass: bool,
    invalidated: bool,
}
struct Styles<'a, I: Iterator<Item = Event>, F> {
    prefix: &'a mut Prefix,
    loader: Option<(&'a Loader, u64)>,
    index: usize,
    fresh: Option<F>,
    input: Option<std::iter::Skip<I>>,
}
impl<I: Iterator<Item = Event>, F: FnOnce() -> I> Iterator for Styles<'_, I, F> {
    type Item = Event;
    fn next(&mut self) -> Option<Event> {
        if let Some(event) = self.prefix.events.get(self.index).copied() {
            self.index += 1;
            return Some(event);
        }
        if self.prefix.exhausted {
            return None;
        }
        // Recreate the highlighter only when replay reaches the end of the
        // prefix. Skipping its old events restores enclosing syntax scopes.
        let input = self
            .input
            .get_or_insert_with(|| self.fresh.take().unwrap()().skip(self.index));
        match input.next() {
            Some(event) => {
                if !self.prefix.bypass {
                    if self.prefix.events.len() < MAX_EVENTS {
                        self.prefix.events.push(event);
                    } else {
                        self.prefix.events = Vec::new();
                        self.prefix.bypass = true;
                    }
                }
                self.index += 1;
                Some(event)
            }
            None => {
                if !self.prefix.bypass {
                    self.prefix.exhausted = true;
                }
                None
            }
        }
    }
}
impl<I: Iterator<Item = Event>, F> Drop for Styles<'_, I, F> {
    fn drop(&mut self) {
        if self
            .loader
            .is_some_and(|(loader, revision)| loader.highlight_revision() != revision)
        {
            self.prefix.events = Vec::new();
            self.prefix.invalidated = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    fn stream<'a, I: Iterator<Item = Event>, F: FnOnce() -> I>(
        prefix: &'a mut Prefix,
        fresh: F,
    ) -> Styles<'a, I, F> {
        Styles {
            prefix,
            loader: None,
            index: 0,
            fresh: Some(fresh),
            input: None,
        }
    }
    #[test]
    fn partial_hits_are_lazy_and_longer_reads_extend_in_order() {
        let mut prefix = Prefix::default();
        let calls = Cell::new(0);
        let events = [
            (0, Style::default()),
            (0, Style::default()),
            (3, Style::default()),
        ];
        assert_eq!(
            stream(&mut prefix, || events
                .into_iter()
                .inspect(|_| calls.set(calls.get() + 1)))
            .take(1)
            .count(),
            1
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(
            stream(&mut prefix, || -> std::array::IntoIter<Event, 3> {
                panic!("a cached prefix needs no highlighter")
            })
            .take(1)
            .count(),
            1
        );
        assert_eq!(
            stream(&mut prefix, || events.into_iter()).collect::<Vec<_>>(),
            events
        );
        assert!(prefix.exhausted);
        assert_eq!(
            stream(&mut prefix, || -> std::array::IntoIter<Event, 3> {
                panic!("exhausted")
            })
            .collect::<Vec<_>>(),
            events
        );
    }
    #[test]
    fn reaching_the_bound_bypasses_retention_without_truncating_output() {
        let mut prefix = Prefix::default();
        assert_eq!(
            stream(&mut prefix, || (0..MAX_EVENTS + 2)
                .map(|i| (i, Style::default())))
            .take(MAX_EVENTS)
            .count(),
            MAX_EVENTS
        );
        assert!(!prefix.bypass);
        assert!(stream(&mut prefix, || (0..MAX_EVENTS + 2)
            .map(|i| (i, Style::default())))
        .map(|event| event.0)
        .eq(0..MAX_EVENTS + 2));
        assert!(prefix.bypass);
        assert!(prefix.events.is_empty());
        assert_eq!(prefix.events.capacity(), 0, "bypass must release storage");
        assert_eq!(
            stream(&mut prefix, || (0..5).map(|i| (i, Style::default()))).count(),
            5
        );
        assert_eq!(prefix.events.capacity(), 0);
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use arc_swap::ArcSwap;
    use editor_core::Rope;
    use std::sync::Arc;
    use view::{graphics::Rect, View};
    fn fixture() -> (Document, View) {
        let mut doc = Document::from(
            Rope::from_str("text\n"),
            None,
            Arc::new(ArcSwap::from_pointee(editor_config::Config::default())),
            Arc::new(Loader::default()),
        );
        let mut view = View::new(doc.id(), Default::default(), Default::default());
        view.area = Rect::new(0, 0, 80, 24);
        doc.ensure_view_init(view.id);
        (doc, view)
    }
    #[tokio::test]
    async fn equal_numeric_versions_and_document_ids_do_not_alias_other_buffers() {
        let (first, view) = fixture();
        let (second, other) = fixture();
        assert_eq!(first.id(), second.id());
        let mut cache = ViewportSyntaxCache::default();
        let one = (0, Style::default());
        let two = (1, Style::default());
        assert_eq!(
            cache
                .styles((&first).into(), &view.layout(&first, None), 0, || [one]
                    .into_iter())
                .collect::<Vec<_>>(),
            [one]
        );
        assert_eq!(
            cache
                .styles((&second).into(), &other.layout(&second, None), 0, || [two]
                    .into_iter())
                .collect::<Vec<_>>(),
            [two]
        );
    }
    #[tokio::test]
    async fn theme_and_in_place_highlight_refresh_invalidate_the_prefix() {
        let (doc, view) = fixture();
        let mut cache = ViewportSyntaxCache::default();
        let layout = view.layout(&doc, None);
        cache
            .styles((&doc).into(), &layout, 0, || {
                [(0, Style::default())].into_iter()
            })
            .for_each(drop);
        assert_eq!(
            cache
                .styles((&doc).into(), &layout, 1, || [(1, Style::default())]
                    .into_iter())
                .next()
                .unwrap()
                .0,
            1
        );
        doc.syntax_loader().refresh_highlights();
        assert_eq!(
            cache
                .styles((&doc).into(), &layout, 1, || [(2, Style::default())]
                    .into_iter())
                .next()
                .unwrap()
                .0,
            2
        );
    }
    #[tokio::test]
    async fn configuration_changes_during_fill_discard_the_recorded_prefix() {
        let (doc, view) = fixture();
        let mut cache = ViewportSyntaxCache::default();
        let layout = view.layout(&doc, None);
        {
            let mut styles = cache.styles((&doc).into(), &layout, 0, || {
                [(0, Style::default())].into_iter()
            });
            assert!(styles.next().is_some());
            doc.syntax_loader().refresh_highlights();
        }
        assert!(cache.entry.as_ref().unwrap().prefix.invalidated);
        assert_eq!(cache.entry.as_ref().unwrap().prefix.events.capacity(), 0);
        assert_eq!(
            cache
                .styles((&doc).into(), &layout, 0, || [(2, Style::default())]
                    .into_iter())
                .next()
                .unwrap()
                .0,
            2
        );
    }

    #[tokio::test]
    async fn equal_length_edits_and_undo_invalidate_cached_styles() {
        let (mut doc, view) = fixture();
        let mut cache = ViewportSyntaxCache::default();
        let read = |cache: &mut ViewportSyntaxCache, doc: &Document, value| {
            cache
                .styles(doc.into(), &view.layout(doc, None), 0, || {
                    [(value, Style::default())].into_iter()
                })
                .next()
                .unwrap()
                .0
        };
        assert_eq!(read(&mut cache, &doc, 0), 0);
        let initial = doc.text().clone();
        let change =
            editor_core::Transaction::change(doc.text(), [(2, 3, Some("s".into()))].into_iter());
        view::editing::apply(&mut doc, &change, view.id).unwrap();
        assert_eq!(read(&mut cache, &doc, 1), 1);
        assert!(doc.undo());
        assert_eq!(doc.text(), &initial);
        assert_eq!(read(&mut cache, &doc, 2), 2);
    }
}

#[cfg(test)]
mod rendering_tests {
    use super::*;
    use crate::ui::{
        document::{render_document, render_document_with_styles},
        highlights,
        text_decorations::DecorationManager,
    };
    use arc_swap::ArcSwap;
    use editor_core::Selection;
    use std::sync::Arc;
    use tui::buffer::Buffer;
    use view::{editor::Action, graphics::Rect, handlers::Handlers, Editor};

    fn draw(editor: &Editor, cache: &mut ViewportSyntaxCache) -> Buffer {
        let (view, doc) = view::current_ref!(editor);
        let area = view.area;
        let mut actual = Buffer::empty(area);
        let mut expected = Buffer::empty(area);
        let config = editor.config();
        let overlays = || {
            vec![highlights::selection_highlights(
                editor.mode(),
                doc.into(),
                view,
                &editor.theme,
                &config.cursor,
                true,
                true,
            )]
        };
        let layout = view.layout(doc, Some(&editor.theme));
        let styles = cache.styles(doc.into(), &layout, editor.theme.revision(), || {
            highlights::document_syntax_styles(doc.into(), &layout, &editor.theme)
        });
        render_document_with_styles(
            &mut actual,
            doc.into(),
            &layout,
            overlays(),
            &editor.theme,
            DecorationManager::default(),
            styles,
        );
        let fresh = view.layout(doc, Some(&editor.theme));
        render_document(
            &mut expected,
            doc.into(),
            &fresh,
            overlays(),
            &editor.theme,
            DecorationManager::default(),
        );
        assert_eq!(
            actual, expected,
            "cached painting must match a fresh highlighter"
        );
        actual
    }

    #[tokio::test(flavor = "multi_thread")]
    #[cfg_attr(
        not(feature = "runtime-grammars"),
        ignore = "requires compiled runtime grammars; enable the runtime-grammars feature"
    )]
    async fn publication_and_geometry_changes_match_fresh_rendering() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("source.rs");
        std::fs::write(&path,"fn first() {\n\tlet café = \"日本語 abcdefghijklmnopqrstuvwxyz abcdefghijklmnopqrstuvwxyz\";\n    println!(\"{}\", café);\n}\nfn second() {}\n")?;
        let mut config = editor_config::Config::default();
        config.lsp.enable = false;
        config.files.watch.enable = false;
        config.files.reload.enable = false;
        config.completion.word.enable = false;
        let sender =
            view::callbacks::EditorCallbackSender::new(Default::default(), |_| async {}, |_| {});
        let handlers = Handlers::new(&config, sender);
        let config = Arc::new(ArcSwap::from_pointee(config));
        let loader = language::loader::default_lang_loader(loader::syntax::Resources::default());
        let themes = Arc::new(theme::Loader::new(loader::theme::Resources::new(vec![])));
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 12),
            themes,
            Arc::new(ArcSwap::from_pointee(loader)),
            config.clone(),
            handlers,
            workspace::trust::WorkspaceTrust::fully_trusted(dir.path().join("trust.toml")),
        );
        let source = r##""ui.text" = "#ffffff"
"ui.selection" = { bg = "#112233" }
"function" = "#ff0000"
"keyword" = "#abcdef"
"string" = "#bbcccc"
"variable" = "#dddddd"
"variable.parameter" = "#ddeeff"
"comment" = "#778899"
"##;
        let first_theme = theme::Theme::from(toml::from_str::<toml::Value>(source)?);
        let second_theme = theme::Theme::from(toml::from_str::<toml::Value>(
            &source.replace("#ff0000", "#00ff00"),
        )?);
        editor.set_theme(first_theme.clone())?;
        let id = editor.open(&path, Action::VerticalSplit)?;
        let view = editor.tree.focus();
        editor.document_mut(id).unwrap().cancel_syntax_request();
        let request = editor.document_mut(id).unwrap().syntax_request().unwrap();
        let version = editor.document(id).unwrap().snapshot().version();
        let mut cache = ViewportSyntaxCache::default();
        let unparsed = draw(&editor, &mut cache);
        let first = request.compute_first().await.unwrap()?;
        let first = first
            .syntax
            .expect("the installed Rust grammar is required");
        let first_revision = first.revision();
        request.publish_first(editor.document_mut(id).unwrap(), first);
        let parsed = draw(&editor, &mut cache);
        assert_ne!(
            unparsed, parsed,
            "publication must replace plain text styles at the same text version"
        );
        let complete = request.compute().await.unwrap()?;
        assert!(request.complete(editor.document_mut(id).unwrap(), Ok(complete)));
        assert_eq!(editor.document(id).unwrap().snapshot().version(), version);
        assert_ne!(
            editor.document(id).unwrap().syntax().unwrap().revision(),
            first_revision
        );
        draw(&editor, &mut cache);
        // Direct public replacement, including an equal theme name, must invalidate too.
        let before = draw(&editor, &mut cache);
        editor.theme = second_theme;
        let after = draw(&editor, &mut cache);
        assert_ne!(before, after);
        editor.set_theme_preview(first_theme)?;
        draw(&editor, &mut cache);
        editor.unset_theme_preview()?;
        draw(&editor, &mut cache);
        for (width, height, wrap, offset) in [
            (80, 12, false, 0),
            (30, 8, true, 0),
            (20, 12, true, 0),
            (35, 10, false, 12),
            (80, 12, false, 0),
        ] {
            let mut updated = (**config.load()).clone();
            updated.soft_wrap.enable = Some(wrap);
            config.store(Arc::new(updated));
            editor.tree.get_mut(view).area = Rect::new(0, 0, width, height);
            let doc = editor.document_mut(id).unwrap();
            let mut position = doc.view_offset(view);
            position.horizontal_offset = offset;
            doc.set_view_offset(view, position);
            for point in [0, 18, 30] {
                editor
                    .document_mut(id)
                    .unwrap()
                    .set_selection(view, Selection::point(point));
                draw(&editor, &mut cache);
            }
        }
        editor
            .document_mut(id)
            .unwrap()
            .set_selection(view, Selection::point(0));
        editor
            .document_mut(id)
            .unwrap()
            .fold(view, view::document::FoldAction::CloseAll);
        draw(&editor, &mut cache);
        editor
            .document_mut(id)
            .unwrap()
            .fold(view, view::document::FoldAction::OpenAll);
        draw(&editor, &mut cache);
        Ok(())
    }
}
