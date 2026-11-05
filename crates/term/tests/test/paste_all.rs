//! `paste_after_all` and `paste_before_all` paste every yanked value at each
//! selection, keeping one selection per pasted value.
use editor_core::hashmap;
use term::keymap;
use view::document::Mode;

use super::helpers::{test_config, test_with_config, AppBuilder};

/// The default keymap with `F1` bound to `paste_after_all` and `F2` to `paste_before_all`.
fn app() -> AppBuilder {
    let mut config = test_config();
    for mode in [Mode::Normal, Mode::Select] {
        config
            .keys
            .get_mut(&mode)
            .unwrap()
            .merge_nodes(keymap!({ "Paste all"
                "F1" => paste_after_all,
                "F2" => paste_before_all,
            }));
    }
    AppBuilder::new().with_config(config)
}

#[tokio::test(flavor = "multi_thread")]
async fn every_yanked_value_is_pasted_and_selected_separately() -> anyhow::Result<()> {
    // Yank both words, keep one selection, then paste both after it.
    test_with_config(
        app(),
        (
            "#[one|]# two #(three|)#\n",
            "y,<F1>",
            "one#[one|]#\n#(three|)# two three\n",
        ),
    )
    .await?;
    test_with_config(
        app(),
        (
            "#[one|]# two #(three|)#\n",
            "y,<F2>",
            "#[one|]#\n#(three|)#one two three\n",
        ),
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn each_selection_receives_all_values() -> anyhow::Result<()> {
    test_with_config(
        app(),
        (
            "#[a|]# #(b|)#\n",
            "y<F1>",
            "a#[a|]#\n#(b|)# b#(a|)#\n#(b|)#\n",
        ),
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linewise_values_paste_as_adjacent_lines_without_merging() -> anyhow::Result<()> {
    test_with_config(
        app(),
        ("#[a\n|]##(b\n|)#c\n", "y,<F1>", "a\n#[a\n|]##(b\n|)#b\nc\n"),
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn count_register_and_undo_work_like_paste() -> anyhow::Result<()> {
    // A count repeats each value, `"a` picks the register, and one `u` undoes the paste.
    test_with_config(
        app(),
        ("#[x|]# #(y|)#\n", "\"ay,\"a2<F1>", "x#[xx|]#\n#(yy|)# y\n"),
    )
    .await?;
    test_with_config(app(), ("#[x|]# #(y|)#\n", "y,<F1>u", "#[x|]# y\n")).await?;
    // Like `p`, select mode keeps the existing selection and returns to normal
    // mode, so `l` then moves the cursor instead of extending the selection.
    test_with_config(app(), ("#[x|]# #(y|)#\n", "y,v<F1>l", "x#[x|]#\ny y\n")).await?;
    Ok(())
}
