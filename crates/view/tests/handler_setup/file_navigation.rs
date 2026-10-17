use super::Fixture;
use view::{
    current, current_ref,
    features::document_links::{prepare_navigation, FileNavigation, NavigationTarget},
};

fn select_path(fixture: &mut Fixture) {
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(
        view.id,
        editor_core::Selection::single(0, doc.text().len_chars() - 1),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn literal_file_navigation_is_available_without_a_frontend_or_language_server(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("target.words\n")?;
    select_path(&mut fixture);
    let directory = current_ref!(fixture.editor)
        .1
        .path()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let FileNavigation::Ready(update) = prepare_navigation(&mut fixture.editor) else {
        panic!("literal paths require no asynchronous resolution");
    };
    assert_eq!(
        update.into_targets(&fixture.editor),
        Some(vec![NavigationTarget::Path(directory.join("target.words"))])
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_navigation_updates_remain_bound_to_their_editor() -> anyhow::Result<()> {
    let mut first = Fixture::new("target.words\n")?;
    let mut other = Fixture::new("target.words\n")?;
    let path = current_ref!(first.editor).1.path().unwrap().to_owned();
    let id = current_ref!(other.editor).1.id();
    other.editor.set_doc_path(id, &path);
    select_path(&mut first);
    select_path(&mut other);
    let FileNavigation::Ready(update) = prepare_navigation(&mut first.editor) else {
        panic!("literal paths require no asynchronous resolution");
    };
    assert!(update.into_targets(&other.editor).is_none());
    Ok(())
}
