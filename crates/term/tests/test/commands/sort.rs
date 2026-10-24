use super::*;

fn selected(lines: &[&str]) -> String {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("#[{line}|]#")
            } else {
                format!("#({line}|)#")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn sort_modes_and_flag_aliases() -> anyhow::Result<()> {
    for (flags, input, expected) in [
        ("", vec!["10", "2", "a"], vec!["10", "2", "a"]),
        (
            "--numeric",
            vec!["10", "2", "-3", "text"],
            vec!["-3", "text", "2", "10"],
        ),
        (
            "-n -r",
            vec!["10", "2", "-3", "text"],
            vec!["10", "2", "text", "-3"],
        ),
        (
            "--natural",
            vec!["file10", "file2", "file1"],
            vec!["file1", "file2", "file10"],
        ),
        ("-N -i -r", vec!["b2", "A2", "a10"], vec!["b2", "a10", "A2"]),
        (
            "--length",
            vec!["界界", "abc", "🙂"],
            vec!["🙂", "界界", "abc"],
        ),
        (
            "-l -r",
            vec!["界界", "abc", "🙂"],
            vec!["abc", "界界", "🙂"],
        ),
        (
            "--ignore-leading-blanks --insensitive",
            vec!["  b", "a", "\tA"],
            vec!["a", "\tA", "  b"],
        ),
        ("-b -i", vec!["  b", "a", "\tA"], vec!["a", "\tA", "  b"]),
        (
            "--key '\\d+' -n",
            vec!["header", "z10", "gap", "a2"],
            vec!["header", "a2", "gap", "z10"],
        ),
        (
            "-k '\\d+' --numeric --reverse",
            vec!["z2", "a10", "b1"],
            vec!["a10", "z2", "b1"],
        ),
    ] {
        test((
            selected(&input),
            format!(":sort {flags}<ret>"),
            selected(&expected),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sort_validation_errors_do_not_edit_or_move_selections() -> anyhow::Result<()> {
    for flags in [
        "-n -N",
        "--numeric --length",
        "--natural --length",
        "--key '['",
        "--key",
    ] {
        let input = selected(&["b", "a"]);
        let mut app = AppBuilder::new().with_input_text(&input).build()?;
        let expected = editor_core::test::print(&input);
        test_key_sequence(
            &mut app,
            Some(&format!(":sort {flags}<ret>")),
            Some(&|app| {
                let (view, doc) = view::current_ref!(app.editor);
                assert_eq!(
                    doc.text(),
                    &editor_core::Rope::from(expected.0.as_str()),
                    "{flags}"
                );
                assert_eq!(doc.selection(view.id), &expected.1, "{flags}");
                assert!(
                    app.editor.is_err(),
                    "{flags}: {:?}",
                    app.editor.get_status()
                );
            }),
            false,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sort_is_one_undoable_edit() -> anyhow::Result<()> {
    let input = selected(&["item10", "item2", "item1"]);
    test((&input, ":sort -N<ret>u", &input, LineFeedHandling::AsIs)).await?;
    Ok(())
}
