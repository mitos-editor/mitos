use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_move_parent_node_end() -> anyhow::Result<()> {
    let tests = vec![
        ("fn f() { #[x|]#; }\n", "<A-e>", "fn f() { x#[;|]# }\n"),
        (
            "fn f() { #[x|]#; }\n",
            "v<A-e><A-e>",
            "fn f() { #[x; |]#}\n",
        ),
    ];

    for test in tests {
        test_with_config(AppBuilder::new().with_file("foo.rs", None), test).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_move_parent_node_start() -> anyhow::Result<()> {
    let tests = vec![
        ("fn f() { #[x|]#; }\n", "<A-b>", "fn f() #[{|]# x; }\n"),
        (
            "fn f() { #[x|]#; }\n",
            "v<A-b><A-b>",
            "#[|fn f() { ]#x; }\n",
        ),
    ];

    for test in tests {
        test_with_config(AppBuilder::new().with_file("foo.rs", None), test).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_smart_tab_move_parent_node_end() -> anyhow::Result<()> {
    let tests = vec![
        // single cursor stays single cursor, first goes to end of current
        // node, then parent
        (
            indoc! {r##"
                fn foo() {
                    let result = if true {
                        "yes"
                    } else {
                        "no#["|]#
                    }
                }
            "##},
            "i<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    } else {
                        \"no\"#[|\n]#
                    }
                }
            "},
        ),
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    } else {
                        \"no\"#[\n|]#
                    }
                }
            "},
            "i<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    } else {
                        \"no\"
                    }#[|\n]#
                }
            "},
        ),
        // appending to the end of a line should still look at the current
        // line, not the next one
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    } else {
                        \"no#[\"|]#
                    }
                }
            "},
            "a<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    } else {
                        \"no\"
                    }#[\n|]#
                }
            "},
        ),
        // before cursor is all whitespace, so insert tab
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    } else {
                        #[\"no\"|]#
                    }
                }
            "},
            "i<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    } else {
                            #[|\"no\"]#
                    }
                }
            "},
        ),
        // if selection spans multiple lines, it should still only look at the
        // line on which the head is
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        #[\"yes\"
                    } else {
                        \"no\"|]#
                    }
                }
            "},
            "a<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    } else {
                        \"no\"
                    }#[\n|]#
                }
            "},
        ),
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        #[\"yes\"
                    } else {
                        \"no\"|]#
                    }
                }
            "},
            "i<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                            #[|\"yes\"
                    } else {
                        \"no\"]#
                    }
                }
            "},
        ),
        (
            indoc! {"\
                fn foo() {
                    #[l|]#et result = if true {
                        #(\"yes\"
                    } else {
                        \"no\"|)#
                    }
                }
            "},
            "i<tab>",
            indoc! {"\
                fn foo() {
                        #[|l]#et result = if true {
                            #(|\"yes\"
                    } else {
                        \"no\")#
                    }
                }
            "},
        ),
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"#[\n|]#
                    } else {
                        \"no\"#(\n|)#
                    }
                }
            "},
            "i<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    }#[| ]#else {
                        \"no\"
                    }#(|\n)#
                }
            "},
        ),
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        #[\"yes\"|]#
                    } else {
                        #(\"no\"|)#
                    }
                }
            "},
            "i<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                            #[|\"yes\"]#
                    } else {
                            #(|\"no\")#
                    }
                }
            "},
        ),
        // if any cursors are not preceded by all whitespace, then do the
        // smart_tab action
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        #[\"yes\"\n|]#
                    } else {
                        \"no#(\"\n|)#
                    }
                }
            "},
            "i<tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                        \"yes\"
                    }#[| ]#else {
                        \"no\"
                    }#(|\n)#
                }
            "},
        ),
        // Ctrl-tab always inserts a tab
        (
            indoc! {"\
                fn foo() {
                    let result = if true {
                        #[\"yes\"\n|]#
                    } else {
                        \"no#(\"\n|)#
                    }
                }
            "},
            "i<S-tab>",
            indoc! {"\
                fn foo() {
                    let result = if true {
                            #[|\"yes\"\n]#
                    } else {
                        \"no #(|\"\n)#
                    }
                }
            "},
        ),
    ];

    for test in tests {
        test_with_config(AppBuilder::new().with_file("foo.rs", None), test).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn select_all_siblings() -> anyhow::Result<()> {
    let tests = vec![
        (
            "let foo = bar(#[a|]#, b, c);\n",
            "<A-a>",
            "let foo = bar(#[a|]#, #(b|)#, #(c|)#);\n",
        ),
        (
            "let a = [#[|1]#, 2, 3];\n",
            "<A-a>",
            "let a = [#[|1]#, #(|2)#, #(|3)#];\n",
        ),
    ];

    for test in tests {
        test_with_config(AppBuilder::new().with_file("foo.rs", None), test).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn select_all_children() -> anyhow::Result<()> {
    let tests = vec![
        (
            "let foo = bar#[(a, b, c)|]#;\n",
            "<A-I>",
            "let foo = bar(#[a|]#, #(b|)#, #(c|)#);\n",
        ),
        (
            "let a = #[|[1, 2, 3]]#;\n",
            "<A-I>",
            "let a = [#[|1]#, #(|2)#, #(|3)#];\n",
        ),
    ];

    for test in tests {
        test_with_config(AppBuilder::new().with_file("foo.rs", None), test).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_select_next_sibling() -> anyhow::Result<()> {
    let tests = vec![
        (
            "fn inc(x: usize) -> usize { x + 1 #[}|]# fn dec(x: usize) -> usize { x - 1 } fn ident(x: usize) -> usize { x }\n",
            "<A-n>",
            "fn inc(x: usize) -> usize { x + 1 } #[fn dec(x: usize) -> usize { x - 1 }|]# fn ident(x: usize) -> usize { x }\n",
        ),
        (
            "fn inc(x: usize) -> usize { x + 1 #[}|]# fn dec(x: usize) -> usize { x - 1 } fn ident(x: usize) -> usize { x }\n",
            "<A-n><A-;><A-n>",
            "fn inc(x: usize) -> usize { x + 1 } fn dec(x: usize) -> usize { x - 1 } #[fn ident(x: usize) -> usize { x }|]#\n",
        ),
    ];

    for test in tests {
        test_with_config(AppBuilder::new().with_file("foo.rs", None), test).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_select_prev_sibling() -> anyhow::Result<()> {
    let tests = vec![
        (
            "fn inc(x: usize) -> usize { x + 1 } fn dec(x: usize) -> usize { x - 1 } #[|f]#n ident(x: usize) -> usize { x }\n",
            "<A-p>",
            "fn inc(x: usize) -> usize { x + 1 } #[|fn dec(x: usize) -> usize { x - 1 }]# fn ident(x: usize) -> usize { x }\n",
        ),
        (
            "fn inc(x: usize) -> usize { x + 1 } fn dec(x: usize) -> usize { x - 1 } #[|f]#n ident(x: usize) -> usize { x }\n",
            "<A-p><A-;><A-p>",
            "#[|fn inc(x: usize) -> usize { x + 1 }]# fn dec(x: usize) -> usize { x - 1 } fn ident(x: usize) -> usize { x }\n",
        ),
    ];

    for test in tests {
        test_with_config(AppBuilder::new().with_file("foo.rs", None), test).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn match_bracket() -> anyhow::Result<()> {
    let rust_tests = vec![
        // fwd
        (
            indoc! {r##"
                fn foo(x: usize) -> usize { #[x|]# + 1 }
            "##},
            "mm",
            indoc! {r##"
                fn foo(x: usize) -> usize { x + 1 #[}|]#
            "##},
        ),
        // backward
        (
            indoc! {r##"
                fn foo(x: usize) -> usize { #[x|]# + 1 }
            "##},
            "mmmm",
            indoc! {r##"
                fn foo(x: usize) -> usize #[{|]# x + 1 }
            "##},
        ),
        // avoid false positive inside string literal
        (
            indoc! {r##"
                fn foo() -> &'static str { "(hello#[ |]#world)" }
            "##},
            "mm",
            indoc! {r##"
                fn foo() -> &'static str { "(hello world)#["|]# }
            "##},
        ),
        // make sure matching on quotes works
        (
            indoc! {r##"
                fn foo() -> &'static str { "(hello#[ |]#world)" }
            "##},
            "mm",
            indoc! {r##"
                fn foo() -> &'static str { "(hello world)#["|]# }
            "##},
        ),
        // .. on both ends
        (
            indoc! {r##"
                fn foo() -> &'static str { "(hello#[ |]#world)" }
            "##},
            "mmmm",
            indoc! {r##"
                fn foo() -> &'static str { #["|]#(hello world)" }
            "##},
        ),
        // match on siblings nodes
        (
            indoc! {r##"
                fn foo(bar: Option<usize>) -> usize {
                    match bar {
                        Some(b#[a|]#r) => bar,
                        None => 42,
                    } 
                }
            "##},
            "mmmm",
            indoc! {r##"
                fn foo(bar: Option<usize>) -> usize {
                    match bar {
                        Some#[(|]#bar) => bar,
                        None => 42,
                    } 
                }
            "##},
        ),
        // gracefully handle multiple sibling brackets (usally for errors/incomplete syntax trees)
        // in the past we selected the first > instead of the second > here
        (
            indoc! {r##"
                fn foo() {
                    foo::<b#[a|]#r<>> 
                }
            "##},
            "mm",
            indoc! {r##"
                fn foo() {
                    foo::<bar<>#[>|]# 
                }
            "##},
        ),
        // named node with 2 or more children
        (
            indoc! {r##"
                use a::#[{|]#
                    b::{c, d, e, f, g},
                    h, i, j, k, l, m, n,
                };
            "##},
            "mm",
            indoc! {r##"
                use a::{
                    b::{c, d, e, f, g},
                    h, i, j, k, l, m, n,
                #[}|]#;
            "##},
        ),
    ];

    let python_tests = vec![
        // python quotes have a slightly more complex syntax tree
        // that triggerd a bug in an old implementation so we test
        // them here
        (
            indoc! {r##"
                foo_python = "mm does not#[ |]#work on this string"
            "##},
            "mm",
            indoc! {r##"
                foo_python = "mm does not work on this string#["|]#
            "##},
        ),
        (
            indoc! {r##"
                foo_python = "mm does not#[ |]#work on this string"
            "##},
            "mmmm",
            indoc! {r##"
                foo_python = #["|]#mm does not work on this string"
            "##},
        ),
    ];

    for test in rust_tests {
        println!("{test:?}");
        test_with_config(AppBuilder::new().with_file("foo.rs", None), test).await?;
    }
    for test in python_tests {
        println!("{test:?}");
        test_with_config(AppBuilder::new().with_file("foo.py", None), test).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reflow_preserves_a_saved_jump_in_the_middle_of_a_paragraph() -> anyhow::Result<()> {
    test((
        "aaaa bbbb #[c|]#ccc dddd\n",
        "<C-s>mip:reflow 9<ret><C-o>",
        "aaaa bbbb\n#[c|]#ccc dddd\n",
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn reflow_preserves_a_saved_jump_in_a_long_unicode_crlf_paragraph() -> anyhow::Result<()> {
    let mut words: Vec<_> = (0..32).map(|i| format!("w{i:02}")).collect();
    words[8] = "#[界|]#08".into();
    let input = format!("{}\r\n", words.join(" "));
    let expected = format!(
        "{}\r\n",
        words
            .chunks(2)
            .map(|pair| pair.join(" "))
            .collect::<Vec<_>>()
            .join("\r\n")
    );
    test((
        input.clone(),
        ":line-ending crlf<ret><C-s>mip:reflow 8<ret><C-o>",
        expected.clone(),
        LineFeedHandling::AsIs,
    ))
    .await?;
    let original = editor_core::test::print(&input).0;
    let wrapped = editor_core::test::print(&expected).0;
    let mut app = AppBuilder::new().with_input_text(input).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":line-ending crlf<ret>mip:reflow 8<ret>u"),
                Some(&|app: &term::application::TestApplication| {
                    assert_eq!(view::doc!(app.editor).text().to_string(), original);
                }),
            ),
            (
                Some("U"),
                Some(&|app: &term::application::TestApplication| {
                    assert_eq!(view::doc!(app.editor).text().to_string(), wrapped);
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn reflow_maps_multiple_selected_fragments_in_document_coordinates() -> anyhow::Result<()> {
    test((
        "prefix #[one two|]# gap #(三 四|)# suffix\n",
        ":reflow 3<ret>",
        "prefix #[one\ntwo|]# gap #(三\n四|)# suffix\n",
    ))
    .await
}
