use editor_core::{auto_pairs::DEFAULT_PAIRS, hashmap};
use indoc::formatdoc;

use super::*;

const LINE_END: &str = editor_core::NATIVE_LINE_ENDING.as_str();

fn differing_pairs() -> impl Iterator<Item = &'static (char, char)> {
    DEFAULT_PAIRS.iter().filter(|(open, close)| open != close)
}

fn matching_pairs() -> impl Iterator<Item = &'static (char, char)> {
    DEFAULT_PAIRS.iter().filter(|(open, close)| open == close)
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_basic() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test(("#[\n|]#", format!("i{open}"), format!("{open}#[|{close}]#"))).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_whitespace() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS
        .iter()
        .filter(|pair| matches!(pair.0, '(' | '{'))
    {
        test((
            format!("{open}#[|{close}]#"),
            "i ",
            format!("{open} #[| ]#{close}"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_whitespace_multi() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs().filter(|pair| matches!(pair.0, '(' | '{')) {
        test((
            formatdoc! {"\
                {open}#[|{close}]#
                {open}#(|{open})#{close}{close}
                {open}{open}#(|{close}{close})#
                foo#(|\n)#
            "},
            "i ",
            formatdoc! {"\
                {open} #[| ]#{close}
                {open} #(|{open})#{close}{close}
                {open}{open} #(| {close}{close})#
                foo #(|\n)#
            "},
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_whitespace_multi() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs().filter(|pair| matches!(pair.0, '(' | '{')) {
        test((
            formatdoc! {"\
                #[|{open}]#{close}
                #(|{open})#{open}{close}{close}
                #(|{open}{open})#{close}{close}
                #(|foo)#
            "},
            "a ",
            formatdoc! {"\
                #[{open}  |]#{close}
                #({open} {open}|)#{close}{close}
                #({open}{open}  |)#{close}{close}
                #(foo \n|)#
            "},
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_whitespace_no_pair() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        // sanity check - do not insert extra whitespace unless immediately
        // surrounded by a pair
        test((
            format!("{open} #[|{close}]#"),
            "i ",
            format!("{open}  #[|{close}]#"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_whitespace_no_matching_pair() -> anyhow::Result<()> {
    for &(open, _) in differing_pairs() {
        // sanity check - verify whitespace does not insert unless both pairs
        // are matches, i.e. no two different openers
        test((
            format!("{open}#[|{open}]#"),
            "i ",
            format!("{open} #[|{open}]#"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_configured_multi_byte_chars() -> anyhow::Result<()> {
    // NOTE: these are multi-byte Unicode characters
    let pairs = hashmap!('„' => '“', '‚' => '‘', '「' => '」');

    let config = Config {
        editor: editor_config::Config {
            auto_pairs: AutoPairConfig {
                pairs: Some(pairs.clone()),
                ..AutoPairConfig::default()
            },
            ..helpers::test_editor_config()
        },
        ..helpers::test_config()
    };

    for (open, close) in pairs.iter() {
        test_with_config(
            AppBuilder::new().with_config(config.clone()),
            (
                format!("#[{LINE_END}|]#"),
                format!("i{open}"),
                format!("{open}#[|{close}]#{LINE_END}"),
                LineFeedHandling::AsIs,
            ),
        )
        .await?;

        test_with_config(
            AppBuilder::new().with_config(config.clone()),
            (
                format!("{open}#[{close}|]#{LINE_END}"),
                format!("i{close}"),
                format!("{open}{close}#[|{LINE_END}]#"),
                LineFeedHandling::AsIs,
            ),
        )
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_after_word() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs() {
        test((
            format!("foo#[{LINE_END}|]#"),
            format!("i{open}"),
            format!("foo{open}#[|{close}]#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    for &(open, _) in matching_pairs() {
        test((
            format!("foo#[{LINE_END}|]#"),
            format!("i{open}"),
            format!("foo{open}#[|{LINE_END}]#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_before_word() -> anyhow::Result<()> {
    for &(open, _) in DEFAULT_PAIRS {
        test((
            format!("#[f|]#oo{LINE_END}"),
            format!("i{open}"),
            format!("{open}#[|f]#oo{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_before_word_selection() -> anyhow::Result<()> {
    for &(open, _) in DEFAULT_PAIRS {
        test((
            format!("#[foo|]#{LINE_END}"),
            format!("i{open}"),
            format!("{open}#[|foo]#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_before_word_selection_trailing_word() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs() {
        test((
            format!("foo#[ wor|]#{LINE_END}"),
            format!("i{open}"),
            format!("foo{open}#[|{close} wor]#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_closer_selection_trailing_word() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs() {
        test((
            format!("foo{open}#[|{close} wor]#{LINE_END}"),
            format!("i{close}"),
            format!("foo{open}{close}#[| wor]#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_before_eol() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("{LINE_END}#[{LINE_END}|]#"),
            format!("i{open}"),
            format!("{LINE_END}{open}#[|{close}]#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_auto_pairs_disabled() -> anyhow::Result<()> {
    for &(open, _) in DEFAULT_PAIRS {
        test_with_config(
            AppBuilder::new().with_config(Config {
                editor: editor_config::Config {
                    auto_pairs: AutoPairConfig {
                        enable: false,
                        ..AutoPairConfig::default()
                    },
                    ..helpers::test_editor_config()
                },
                ..helpers::test_config()
            }),
            (
                format!("#[{LINE_END}|]#"),
                format!("i{open}"),
                format!("{open}#[|{LINE_END}]#"),
                LineFeedHandling::AsIs,
            ),
        )
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_multi_range() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("#[{LINE_END}|]##({LINE_END}|)##({LINE_END}|)#"),
            format!("i{open}"),
            format!("{open}#[|{close}]#{LINE_END}{open}#(|{close})#{LINE_END}{open}#(|{close})#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_before_multi_code_point_graphemes() -> anyhow::Result<()> {
    for &(_, close) in differing_pairs() {
        test((
            format!("hello #[👨‍👩‍👧‍👦|]# goodbye{LINE_END}"),
            format!("i{close}"),
            format!("hello {close}#[|👨‍👩‍👧‍👦]# goodbye{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_at_end_of_document() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test(TestCase {
            in_text: String::from(LINE_END),
            in_selection: Selection::single(LINE_END.len(), LINE_END.len()),
            in_keys: format!("i{open}"),
            out_text: format!("{LINE_END}{open}{close}"),
            out_selection: Selection::single(LINE_END.len() + 1, LINE_END.len() + 2),
        })
        .await?;

        test(TestCase {
            in_text: format!("foo{LINE_END}"),
            in_selection: Selection::single(3 + LINE_END.len(), 3 + LINE_END.len()),
            in_keys: format!("i{open}"),
            out_text: format!("foo{LINE_END}{open}{close}"),
            out_selection: Selection::single(LINE_END.len() + 4, LINE_END.len() + 5),
        })
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_close_inside_pair() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("{open}#[{close}|]#{LINE_END}"),
            format!("i{close}"),
            format!("{open}{close}#[|{LINE_END}]#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_close_inside_pair_multi() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("{open}#[{close}|]#{LINE_END}{open}#({close}|)#{LINE_END}{open}#({close}|)#{LINE_END}"),
            format!("i{close}"),
            format!("{open}{close}#[|{LINE_END}]#{open}{close}#(|{LINE_END})#{open}{close}#(|{LINE_END})#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_nested_open_inside_pair() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs() {
        test((
            format!("{open}#[{close}|]#{LINE_END}"),
            format!("i{open}"),
            format!("{open}{open}#[|{close}]#{close}{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_nested_open_inside_pair_multi() -> anyhow::Result<()> {
    for &(outer_open, outer_close) in DEFAULT_PAIRS {
        for &(inner_open, inner_close) in DEFAULT_PAIRS {
            if inner_open == outer_open {
                continue;
            }

            test((
                format!("{outer_open}#[{outer_close}|]#{LINE_END}{outer_open}#({outer_close}|)#{LINE_END}{outer_open}#({outer_close}|)#{LINE_END}"),
                format!("i{inner_open}"),
                format!("{outer_open}{inner_open}#[|{inner_close}]#{outer_close}{LINE_END}{outer_open}{inner_open}#(|{inner_close})#{outer_close}{LINE_END}{outer_open}{inner_open}#(|{inner_close})#{outer_close}{LINE_END}"),
                LineFeedHandling::AsIs,
            ))
            .await?;
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_basic() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("#[{LINE_END}|]#"),
            format!("a{open}"),
            format!("#[{LINE_END}{open}{close}|]#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_multi_range() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("#[ |]#{LINE_END}#( |)#{LINE_END}#( |)#{LINE_END}"),
            format!("a{open}"),
            format!("#[ {open}{close}|]#{LINE_END}#( {open}{close}|)#{LINE_END}#( {open}{close}|)#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_close_inside_pair() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("#[{open}|]#{close}{LINE_END}"),
            format!("a{close}"),
            format!("#[{open}{close}{LINE_END}|]#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_close_inside_pair_multi() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("#[{open}|]#{close}{LINE_END}#({open}|)#{close}{LINE_END}#({open}|)#{close}{LINE_END}"),
            format!("a{close}"),
            format!("#[{open}{close}{LINE_END}|]##({open}{close}{LINE_END}|)##({open}{close}{LINE_END}|)#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_end_of_word() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs() {
        test((
            format!("fo#[o|]#{LINE_END}"),
            format!("a{open}"),
            format!("fo#[o{open}{close}|]#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_middle_of_word() -> anyhow::Result<()> {
    for &(_, close) in differing_pairs() {
        test((
            format!("#[wo|]#rd{LINE_END}"),
            format!("a{close}"),
            format!("#[wo{close}r|]#d{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_end_of_word_multi() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs() {
        test((
            format!("fo#[o|]#{LINE_END}fo#(o|)#{LINE_END}fo#(o|)#{LINE_END}"),
            format!("a{open}"),
            format!("fo#[o{open}{close}|]#{LINE_END}fo#(o{open}{close}|)#{LINE_END}fo#(o{open}{close}|)#{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_inside_nested_pair() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs() {
        test((
            format!("f#[oo{open}|]#{close}{LINE_END}"),
            format!("a{open}"),
            format!("f#[oo{open}{open}{close}|]#{close}{LINE_END}"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_inside_nested_pair_multi() -> anyhow::Result<()> {
    for &(outer_open, outer_close) in DEFAULT_PAIRS {
        for &(inner_open, inner_close) in DEFAULT_PAIRS {
            if inner_open == outer_open {
                continue;
            }

            test((
                format!("f#[oo{outer_open}|]#{outer_close}{LINE_END}f#(oo{outer_open}|)#{outer_close}{LINE_END}f#(oo{outer_open}|)#{outer_close}{LINE_END}"),
                format!("a{inner_open}"),
                format!("f#[oo{outer_open}{inner_open}{inner_close}|]#{outer_close}{LINE_END}f#(oo{outer_open}{inner_open}{inner_close}|)#{outer_close}{LINE_END}f#(oo{outer_open}{inner_open}{inner_close}|)#{outer_close}{LINE_END}"),
                LineFeedHandling::AsIs,
            ))
            .await?;
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_basic() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("{open}#[|{close}]#{LINE_END}"),
            "i<backspace>",
            format!("#[|{LINE_END}]#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_multi() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            formatdoc! {"\
                {open}#[|{close}]#
                {open}#(|{close})#
                {open}#(|{close})#
            "},
            "i<backspace>",
            indoc! {"\
                #[|\n]#
                #(|\n)#
                #(|\n)#
            "},
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_whitespace() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("{open} #[| ]#{close}"),
            "i<backspace>",
            format!("{open}#[|{close}]#"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_whitespace_after_word() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("foo{open} #[| ]#{close}"),
            "i<backspace>",
            format!("foo{open}#[|{close}]#"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_whitespace_multi() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            formatdoc! {"\
                {open} #[| ]#{close}
                {open} #(|{open})#{close}{close}
                {open}{open} #(| {close}{close})#
                foo #(|\n)#
            "},
            "i<backspace>",
            formatdoc! {"\
                {open}#[|{close}]#
                {open}#(|{open})#{close}{close}
                {open}{open}#(|{close}{close})#
                foo#(|\n)#
            "},
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_append_whitespace_multi() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            formatdoc! {"\
                #[{open} |]# {close}
                #({open} |)#{open}{close}{close}
                #({open}{open} |)# {close}{close}
                #(foo |)#
            "},
            "a<backspace>",
            formatdoc! {"\
                #[{open}{close}|]#
                #({open}{open}|)#{close}{close}
                #({open}{open}{close}|)#{close}
                #(foo\n|)#
            "},
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_whitespace_no_pair() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("{open}  #[|{close}]#"),
            "i<backspace>",
            format!("{open} #[|{close}]#"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_whitespace_no_matching_pair() -> anyhow::Result<()> {
    for &(open, _) in differing_pairs() {
        test((
            format!("{open} #[|{open}]#"),
            "i<backspace>",
            format!("{open}#[|{open}]#"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_configured_multi_byte_chars() -> anyhow::Result<()> {
    // NOTE: these are multi-byte Unicode characters
    let pairs = hashmap!('„' => '“', '‚' => '‘', '「' => '」');

    let config = Config {
        editor: editor_config::Config {
            auto_pairs: AutoPairConfig {
                pairs: Some(pairs.clone()),
                ..AutoPairConfig::default()
            },
            ..helpers::test_editor_config()
        },
        ..helpers::test_config()
    };

    for (open, close) in pairs.iter() {
        test_with_config(
            AppBuilder::new().with_config(config.clone()),
            (
                format!("{open}#[|{close}]#{LINE_END}"),
                "i<backspace>",
                format!("#[|{LINE_END}]#"),
                LineFeedHandling::AsIs,
            ),
        )
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_after_word() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            &format!("foo{open}#[|{close}]#"),
            "i<backspace>",
            "foo#[|\n]#",
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_then_delete() -> anyhow::Result<()> {
    for &(open, _) in differing_pairs() {
        test((
            "#[\n|]#\n",
            format!("ofoo{open}<backspace>"),
            "\nfoo#[\n|]#\n",
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_then_delete_whitespace() -> anyhow::Result<()> {
    for &(open, _) in differing_pairs() {
        test((
            "foo#[\n|]#",
            format!("i{open}<space><backspace><backspace>"),
            "foo#[|\n]#",
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_then_delete_multi() -> anyhow::Result<()> {
    for &(open, _) in differing_pairs() {
        test((
            indoc! {"\
                through a day#[\n|]#
                in and out of weeks#(\n|)#
                over a year#(\n|)#
            "},
            format!("i{open}<space><backspace><backspace>"),
            indoc! {"\
                through a day#[|\n]#
                in and out of weeks#(|\n)#
                over a year#(|\n)#
            "},
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_then_delete() -> anyhow::Result<()> {
    for &(open, _) in differing_pairs() {
        test((
            "fo#[o|]#",
            format!("a{open}<space><backspace><backspace>"),
            "fo#[o\n|]#",
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_then_delete_multi() -> anyhow::Result<()> {
    for &(open, _) in differing_pairs() {
        test((
            indoc! {"\
                #[through a day|]#
                #(in and out of weeks|)#
                #(over a year|)#
            "},
            format!("a{open}<space><backspace><backspace>"),
            indoc! {"\
                #[through a day\n|]#
                #(in and out of weeks\n|)#
                #(over a year\n|)#
            "},
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_before_word() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        // sanity check unclosed pair delete
        test((
            format!("{open}#[|f]#oo{LINE_END}"),
            "i<backspace>",
            format!("#[|f]#oo{LINE_END}"),
        ))
        .await?;

        // deleting the closing pair should NOT delete the whole pair
        test((
            format!("{open}{close}#[|f]#oo{LINE_END}"),
            "i<backspace>",
            format!("{open}#[|f]#oo{LINE_END}"),
        ))
        .await?;

        // deleting whole pair before word
        test((
            format!("{open}#[|{close}]#foo{LINE_END}"),
            "i<backspace>",
            format!("#[|f]#oo{LINE_END}"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_before_word_selection() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        // sanity check unclosed pair delete
        test((
            format!("{open}#[|foo]#{LINE_END}"),
            "i<backspace>",
            format!("#[|foo]#{LINE_END}"),
        ))
        .await?;

        // deleting the closing pair should NOT delete the whole pair
        test((
            format!("{open}{close}#[|foo]#{LINE_END}"),
            "i<backspace>",
            format!("{open}#[|foo]#{LINE_END}"),
        ))
        .await?;

        // deleting whole pair before word
        test((
            format!("{open}#[|{close}foo]#{LINE_END}"),
            "i<backspace>",
            format!("#[|foo]#{LINE_END}"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_before_word_selection_trailing_word() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("foo{open}#[|{close} wor]#{LINE_END}"),
            "i<backspace>",
            format!("foo#[| wor]#{LINE_END}"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_before_eol() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("{LINE_END}{open}#[|{close}]#{LINE_END}"),
            "i<backspace>",
            format!("{LINE_END}#[|{LINE_END}]#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_auto_pairs_disabled() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test_with_config(
            AppBuilder::new().with_config(Config {
                editor: editor_config::Config {
                    auto_pairs: AutoPairConfig {
                        enable: false,
                        ..AutoPairConfig::default()
                    },
                    ..helpers::test_editor_config()
                },
                ..helpers::test_config()
            }),
            (
                format!("{open}#[|{close}]#{LINE_END}"),
                "i<backspace>",
                format!("#[|{close}]#{LINE_END}"),
            ),
        )
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_before_multi_code_point_graphemes() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("hello {close}#[|👨‍👩‍👧‍👦]# goodbye{LINE_END}"),
            "i<backspace>",
            format!("hello #[|👨‍👩‍👧‍👦]# goodbye{LINE_END}"),
        ))
        .await?;

        test((
            format!("hello {open}{close}#[|👨‍👩‍👧‍👦]# goodbye{LINE_END}"),
            "i<backspace>",
            format!("hello {open}#[|👨‍👩‍👧‍👦]# goodbye{LINE_END}"),
        ))
        .await?;

        test((
            format!("hello {open}#[|{close}]#👨‍👩‍👧‍👦 goodbye{LINE_END}"),
            "i<backspace>",
            format!("hello #[|👨‍👩‍👧‍👦]# goodbye{LINE_END}"),
        ))
        .await?;

        test((
            format!("hello {open}#[|{close}👨‍👩‍👧‍👦]# goodbye{LINE_END}"),
            "i<backspace>",
            format!("hello #[|👨‍👩‍👧‍👦]# goodbye{LINE_END}"),
        ))
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_at_end_of_document() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test(TestCase {
            in_text: format!("{LINE_END}{open}{close}"),
            in_selection: Selection::single(LINE_END.len() + 1, LINE_END.len() + 2),
            in_keys: String::from("i<backspace>"),
            out_text: String::from(LINE_END),
            out_selection: Selection::single(LINE_END.len(), LINE_END.len()),
        })
        .await?;

        test(TestCase {
            in_text: format!("foo{LINE_END}{open}{close}"),
            in_selection: Selection::single(LINE_END.len() + 4, LINE_END.len() + 5),
            in_keys: String::from("i<backspace>"),
            out_text: format!("foo{LINE_END}"),
            out_selection: Selection::single(3 + LINE_END.len(), 3 + LINE_END.len()),
        })
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_nested_open_inside_pair() -> anyhow::Result<()> {
    for &(open, close) in differing_pairs() {
        test((
            format!("{open}{open}#[|{close}]#{close}{LINE_END}"),
            "i<backspace>",
            format!("{open}#[|{close}]#{LINE_END}"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_nested_open_inside_pair_multi() -> anyhow::Result<()> {
    for &(outer_open, outer_close) in DEFAULT_PAIRS {
        for &(inner_open, inner_close) in DEFAULT_PAIRS {
            if inner_open == outer_open {
                continue;
            }

            test((
                format!("{outer_open}{inner_open}#[|{inner_close}]#{outer_close}{LINE_END}{outer_open}{inner_open}#(|{inner_close})#{outer_close}{LINE_END}{outer_open}{inner_open}#(|{inner_close})#{outer_close}{LINE_END}"),
                "i<backspace>",
                format!("{outer_open}#[|{outer_close}]#{LINE_END}{outer_open}#(|{outer_close})#{LINE_END}{outer_open}#(|{outer_close})#{LINE_END}"),
            ))
            .await?;
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_append_basic() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("#[{LINE_END}{open}|]#{close}{LINE_END}"),
            "a<backspace>",
            format!("#[{LINE_END}{LINE_END}|]#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_append_multi_range() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("#[ {open}|]#{close}{LINE_END}#( {open}|)#{close}{LINE_END}#( {open}|)#{close}{LINE_END}"),
            "a<backspace>",
            format!("#[ {LINE_END}|]##( {LINE_END}|)##( {LINE_END}|)#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_append_end_of_word() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("fo#[o{open}|]#{close}{LINE_END}"),
            "a<backspace>",
            format!("fo#[o{LINE_END}|]#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_mixed_dedent() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            formatdoc! {"\
                bar = {open}#[|{close}]#
                    #(|\n)#
                foo#(|\n)#
            "},
            "i<backspace>",
            indoc! {"\
                bar = #[|\n]#
                #(|\n)#
                fo#(|\n)#
            "},
        ))
        .await?;

        test((
            formatdoc! {"\
                bar = {open}#[|{close}woop]#
                    #(|word)#
                fo#(|o)#
            "},
            "i<backspace>",
            indoc! {"\
                bar = #[|woop]#
                #(|word)#
                f#(|o)#
            "},
        ))
        .await?;

        // delete from the right with append
        test((
            formatdoc! {"\
                bar = #[|woop{open}]#{close}
                #(|    )#word
                #(|fo)#o
            "},
            "a<backspace>",
            indoc! {"\
                bar = #[woop\n|]#
                #(w|)#ord
                #(fo|)#
            "},
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_append_end_of_word_multi() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("fo#[o{open}|]#{close}{LINE_END}fo#(o{open}|)#{close}{LINE_END}fo#(o{open}|)#{close}{LINE_END}"),
            "a<backspace>",
            format!("fo#[o{LINE_END}|]#fo#(o{LINE_END}|)#fo#(o{LINE_END}|)#"),
            LineFeedHandling::AsIs,
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_append_inside_nested_pair() -> anyhow::Result<()> {
    for &(open, close) in DEFAULT_PAIRS {
        test((
            format!("f#[oo{open}{open}|]#{close}{close}{LINE_END}"),
            "a<backspace>",
            format!("f#[oo{open}{close}|]#{LINE_END}"),
        ))
        .await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_append_inside_nested_pair_multi() -> anyhow::Result<()> {
    for &(outer_open, outer_close) in DEFAULT_PAIRS {
        for &(inner_open, inner_close) in DEFAULT_PAIRS {
            if inner_open == outer_open {
                continue;
            }

            test((
                format!("f#[oo{outer_open}{inner_open}|]#{inner_close}{outer_close}{LINE_END}f#(oo{outer_open}{inner_open}|)#{inner_close}{outer_close}{LINE_END}f#(oo{outer_open}{inner_open}|)#{inner_close}{outer_close}{LINE_END}"),
                "a<backspace>",
                format!("f#[oo{outer_open}{outer_close}|]#{LINE_END}f#(oo{outer_open}{outer_close}|)#{LINE_END}f#(oo{outer_open}{outer_close}|)#{LINE_END}"),
            ))
            .await?;
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_pair_closer_with_combining_mark() -> anyhow::Result<()> {
    // `)` + U+0301 is a single 2-char grapheme. `<right>` in insert mode
    // leaves a forward 1-width cursor on it (Forward, single grapheme arm).
    test((
        "#[(|]#)\u{301}\n",
        "i<right><backspace>",
        "#[\n|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // plain `i` gives a backward cursor; closer grapheme at EOF without
    // a trailing newline (Backward, single grapheme arm)
    test((
        "foo(#[|)\u{301}\u{301}]#",
        "i<backspace>",
        "foo#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_whitespace_pair_crlf_forward_cursor() -> anyhow::Result<()> {
    // LF control: passes today
    test((
        "#[(|]#\n\t)\n",
        "i<right><right><backspace>",
        "(#[)|]#\n",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // CRLF: after joining `(\r\n\t)` the cursor must sit on the closer,
    // exactly like with LF
    test((
        "#[(|]#\r\n\t)\r\n",
        "i<right><right><backspace>",
        "(#[)|]#\r\n",
        LineFeedHandling::AsIs,
    ))
    .await?;

    Ok(())
}
