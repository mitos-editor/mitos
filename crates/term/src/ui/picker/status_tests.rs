use super::{
    test::{application, frame, until},
    *,
};

#[tokio::test]
async fn source_status_preserves_search_input_and_result_rows() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("item", |_: &u32, _: &()| "needle".into())],
        0,
        [1],
        (),
        |_, _, _| {},
    )
    .with_preview(|_, _| None)
    .with_editor_updates(|_| None);
    until(&mut picker, &mut app, |p| p.selection() == Some(&1)).await?;
    picker.prompt.set_line("needle".into(), &app.editor);
    let before = frame(&mut picker, &mut app);
    let result_rows = picker.completion_height;
    picker.editor_updates.as_mut().unwrap().status = Some("More history available".into());
    let after = frame(&mut picker, &mut app);
    let row = |surface: &Surface, y| {
        (0..surface.area.width)
            .map(|x| surface[(x, y)].symbol())
            .collect::<String>()
    };
    assert!(row(&before, 1).contains("needle"));
    assert_eq!(row(&after, 1), row(&before, 1));
    assert!(row(&after, 2).contains("More history available"));
    assert_eq!(picker.completion_height, result_rows);
    Ok(())
}
