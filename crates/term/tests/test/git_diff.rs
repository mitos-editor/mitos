use super::helpers::{test_key_sequences, AppBuilder};
use view::current_ref;

#[tokio::test(flavor = "multi_thread")]
async fn diff_command_owns_input_until_closed_and_does_not_save() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "original\n")?;
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":diff<ret>"),
                Some(&|app| {
                    assert!(app
                        .editor
                        .status_msg
                        .as_ref()
                        .is_none_or(|(message, _)| !message.contains("Unknown command")));
                }),
            ),
            // Even a comparison error (this path is outside a repository) leaves
            // a read-only view. Editing input must never reach the document below.
            (
                Some("iSHOULDNOTINSERT"),
                Some(&|app| {
                    assert_eq!(current_ref!(app.editor).1.text().to_string(), "original\n");
                }),
            ),
            (
                Some("<esc>iedited<esc>"),
                Some(&|app| {
                    assert!(current_ref!(app.editor)
                        .1
                        .text()
                        .to_string()
                        .starts_with("edited"));
                }),
            ),
        ],
        false,
    )
    .await?;
    assert_eq!(std::fs::read_to_string(path)?, "original\n");
    Ok(())
}
