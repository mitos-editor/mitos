use editor_core::{syntax::config::AutoPairConfig, Selection};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use view::{
    current_ref,
    editor::Action,
    events::ConfigDidChange,
    theme::{self, Theme},
    view::ViewPosition,
};

use super::Fixture;

#[tokio::test(flavor = "multi_thread")]
async fn language_reload_retains_each_editors_selected_runtime() -> anyhow::Result<()> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    for (dir, content) in [(&first, "; first source"), (&second, "; second source")] {
        let path = dir.path().join("queries/resource-test");
        std::fs::create_dir_all(&path)?;
        std::fs::write(path.join("highlights.scm"), content)?;
    }
    let mut a = Fixture::with_resources(
        "first\n",
        "language = []",
        loader::syntax::Resources::new(vec![first.path().into()]),
    )?;
    let mut b = Fixture::with_resources(
        "second\n",
        "language = []",
        loader::syntax::Resources::new(vec![second.path().into()]),
    )?;
    for (fixture, expected) in [(&mut a, "; first source"), (&mut b, "; second source")] {
        let config = (*fixture.editor.config()).clone();
        let language_loader = fixture.editor.load_language_config(&config)?;
        assert_eq!(
            language_loader
                .resources()
                .query("resource-test", "highlights.scm"),
            expected
        );
        fixture.editor.apply_language_config(
            language_loader,
            theme::Loader::new(loader::theme::Resources::new(vec![])).default_theme(),
        )?;
        let installed = fixture.editor.syn_loader.load();
        assert_eq!(
            installed
                .resources()
                .query("resource-test", "highlights.scm"),
            expected
        );
        // These isolated roots deliberately exclude the process-default native grammars.
        assert!(installed.resources().grammar("json")?.is_none());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_settings_are_visible_to_hooks_and_all_views_are_adjusted_afterward(
) -> anyhow::Result<()> {
    let mut f = Fixture::new(&"line\n".repeat(500))?;
    let path = current_ref!(f.editor).1.path().unwrap().to_path_buf();
    f.editor.open(&path, Action::VerticalSplit)?;
    assert_eq!(f.editor.tree.views().count(), 2);
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    event::register_hook!(move |event: &mut ConfigDidChange<'_>| {
        assert!(!event.old.soft_wrap.enable.unwrap_or(false));
        assert_eq!(event.new.soft_wrap.enable, Some(true));
        assert_eq!(event.editor.config().soft_wrap.enable, Some(true));
        assert!(event
            .editor
            .auto_pairs
            .as_ref()
            .is_none_or(|pairs| pairs.get('(').is_none()));
        // A hook changes selection after the editor's initial layout refresh.
        // The final cursor adjustment must use this selection in every split.
        for (view, _) in event.editor.tree.views() {
            let doc = event.editor.documents.get_mut(&view.doc).unwrap();
            doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 2));
            doc.set_view_offset(view.id, ViewPosition::default());
        }
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let mut settings = (*f.editor.config()).clone();
    settings.soft_wrap.enable = Some(true);
    settings.auto_pairs = AutoPairConfig::Enable(false);
    settings.scrolloff = 3;
    f.configure(|config| *config = settings);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for (view, _) in f.editor.tree.views_mut() {
        let doc = f.editor.documents.get(&view.doc).unwrap();
        assert!(doc.view_offset(view.id).anchor > 0);
        assert!(view.is_cursor_in_view(doc, 3));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_theme_keeps_the_previous_theme_and_still_refreshes_documents() -> anyhow::Result<()>
{
    let mut f = Fixture::new("text\n")?;
    let old_theme = f.editor.theme.scopes().to_vec();
    let loader = editor_core::syntax::Loader::new(
        toml::from_str(
            r#"
        [[language]]
        name = "new-config-test"
        scope = "source.new-config-test"
        file-types = ["words"]
        roots = []
    "#,
        )?,
        loader::syntax::Resources::default(),
    )?;
    let invalid_theme = Theme::from(toml::Value::Table(Default::default()));
    let error = f
        .editor
        .apply_language_config(loader, invalid_theme)
        .unwrap_err();
    assert!(error.to_string().contains("ui.selection"));
    assert_eq!(f.editor.theme.scopes(), old_theme);
    assert_eq!(
        current_ref!(f.editor)
            .1
            .language_config()
            .unwrap()
            .language_id,
        "new-config-test"
    );
    Ok(())
}
