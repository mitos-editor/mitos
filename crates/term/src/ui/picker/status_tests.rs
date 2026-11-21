use super::{
    test::{application, frame, until},
    *,
};

#[tokio::test(start_paused = true)]
async fn picker_spinner_stays_active_while_a_page_is_pending() -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("item", |item: &u32, _: &()| {
            item.to_string().into()
        })],
        0,
        [1],
        (),
        |_, _, _| {},
    );
    until(&mut picker, &mut app, |p| p.selection() == Some(&1)).await?;
    let loaded = Arc::new(tokio::sync::Notify::new());
    let mut picker = picker.with_initial_options({
        let loaded = loaded.clone();
        move |_| {
            let loaded = loaded.clone();
            Box::pin(async move {
                loaded.notified().await;
                Ok(vec![7])
            })
        }
    });
    let screen =
        |surface: Surface| -> String { surface.content.iter().map(|cell| cell.symbol()).collect() };
    let loading = screen(frame(&mut picker, &mut app));
    assert!(loading.contains("Loading results…"), "{loading}");
    assert!(loading.contains("1/⣾"), "{loading}");

    loaded.notify_one();
    until(&mut picker, &mut app, |p| p.selection() == Some(&7)).await?;
    let complete = screen(frame(&mut picker, &mut app));
    assert!(complete.contains("1/1"), "{complete}");
    Ok(())
}

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
    picker.updates.as_mut().unwrap().status = Some("More history available".into());
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

#[tokio::test]
async fn dropping_a_producer_does_not_claim_exhaustion_and_errors_remain_partial(
) -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("item", |item: &u32, _: &()| {
            item.to_string().into()
        })],
        0,
        [],
        (),
        |_, _, _| {},
    );
    let producer = picker.injector();
    producer.push(1)?;
    drop(producer);
    until(&mut picker, &mut app, |p| p.selection() == Some(&1)).await?;
    assert_eq!(picker.results.source.get(), source::State::Loading);
    let surface = frame(&mut picker, &mut app);
    let screen: String = surface.content.iter().map(|cell| cell.symbol()).collect();
    assert!(screen.contains("1/?"));
    assert!(screen.contains("source did not finish"));
    let producer = picker.injector();
    producer.fail();
    producer.complete();
    assert_eq!(picker.results.source.get(), source::State::Failed);
    Ok(())
}

#[tokio::test]
async fn obsolete_producers_cannot_publish_rows_or_completion_into_a_new_query(
) -> anyhow::Result<()> {
    let app = application(false)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("item", |item: &u32, _: &()| {
            item.to_string().into()
        })],
        0,
        [],
        (),
        |_, _, _| {},
    );
    let old = picker.injector();
    picker.results.invalidate();
    let new = picker.injector();
    old.complete();
    old.fail();
    old.pause();
    assert!(old.push(9).is_err());
    assert_eq!(picker.results.source.get(), source::State::Loading);
    new.complete();
    assert_eq!(picker.results.source.get(), source::State::Complete);
    Ok(())
}

#[tokio::test]
async fn streaming_rows_preserve_the_selected_item_when_new_matches_rank_above_it(
) -> anyhow::Result<()> {
    let mut app = application(false)?;
    let mut picker = Picker::new(
        app.editor.redraw().clone(),
        [Column::new("item", |item: &String, _: &()| {
            item.clone().into()
        })],
        0,
        [],
        (),
        |_, _, _| {},
    );
    let producer = picker.injector();
    producer.push("axbyc".into())?;
    producer.push("a___b___c".into())?;
    picker.results.reparse(0, "abc", false);
    until(&mut picker, &mut app, |p| {
        p.results.matcher.snapshot().matched_item_count() == 2
    })
    .await?;
    picker.results.cursor = 1;
    let selected = picker.selection().cloned();
    producer.push("abc".into())?;
    producer.complete();
    until(&mut picker, &mut app, |p| {
        p.results.matcher.snapshot().item_count() == 3
    })
    .await?;
    assert_eq!(picker.selection(), selected.as_ref());
    assert_eq!(picker.results.cursor, 2);
    Ok(())
}
