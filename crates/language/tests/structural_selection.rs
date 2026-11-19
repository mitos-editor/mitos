use editor_core::{
    movement::{Direction, Movement},
    test, Rope,
};
use language::{object, Syntax};

#[derive(Clone, Copy)]
enum Action {
    Parent(Direction, Movement, usize),
    Siblings,
    Children,
    Next(bool),
    Previous(bool),
}

#[test]
fn structural_selection_and_parent_boundaries() {
    let loader = language::loader::default_lang_loader(loader::syntax::Resources::default());
    let language = loader.language_for_name("rust").unwrap();
    let cases = [
        (
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        \"no#[\"|]#\n    }\n}\n",
            Action::Parent(Direction::Forward, Movement::Move, 1),
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        \"no\"#[\n|]#\n    }\n}\n",
        ),
        (
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        \"no\"#[\n|]#\n    }\n}\n",
            Action::Parent(Direction::Forward, Movement::Move, 1),
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        \"no\"\n    }#[\n|]#\n}\n",
        ),
        (
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        #[\"no\"|]#\n    }\n}\n",
            Action::Parent(Direction::Forward, Movement::Extend, 2),
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        #[\"no\"\n    }\n|]#\n}\n",
        ),
        (
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        \"no#[\"|]#\n    }\n}\n",
            Action::Parent(Direction::Backward, Movement::Move, 1),
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        #[\"|]#no\"\n    }\n}\n",
        ),
        (
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        \"no\"#[\n|]#\n    }\n}\n",
            Action::Parent(Direction::Backward, Movement::Move, 1),
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else #[{|]#\n        \"no\"\n    }\n}\n",
        ),
        (
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else #[{|]#\n        \"no\"\n    }\n}\n",
            Action::Parent(Direction::Backward, Movement::Move, 1),
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } #[e|]#lse {\n        \"no\"\n    }\n}\n",
        ),
        (
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        #[\"no\"|]#\n    }\n}\n",
            Action::Parent(Direction::Backward, Movement::Extend, 2),
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else #[|{\n        ]#\"no\"\n    }\n}\n",
        ),
        (
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } else {\n        #[\"no\"|]#\n    }\n}\n",
            Action::Parent(Direction::Backward, Movement::Extend, 3),
            "fn foo() {\n    let result = if true {\n        \"yes\"\n    } #[|else {\n        ]#\"no\"\n    }\n}\n",
        ),
        (
            "let foo = bar(#[a|]#, b, c);\n",
            Action::Siblings,
            "let foo = bar(#[a|]#, #(b|)#, #(c|)#);\n",
        ),
        (
            "let a = [\n    #[1|]#,\n    2,\n    3,\n    4,\n    5,\n];\n",
            Action::Siblings,
            "let a = [\n    #[1|]#,\n    #(2|)#,\n    #(3|)#,\n    #(4|)#,\n    #(5|)#,\n];\n",
        ),
        (
            "let a = [\n    #[|1]#,\n    2,\n    3,\n    4,\n    5,\n];\n",
            Action::Siblings,
            "let a = [\n    #[|1]#,\n    #(|2)#,\n    #(|3)#,\n    #(|4)#,\n    #(|5)#,\n];\n",
        ),
        (
            "let a = [\n    #[1|]#,\n    #(2|)#,\n    #(3|)#,\n    #(4|)#,\n    #(5|)#,\n];\n",
            Action::Siblings,
            "let a = [\n    #[1|]#,\n    #(2|)#,\n    #(3|)#,\n    #(4|)#,\n    #(5|)#,\n];\n",
        ),
        (
            "let a = [\n    #[1|]#,\n    2,\n    3,\n    4,\n    5,\n];\n\nlet b = [\n    #(\"one\"|)#,\n    \"two\",\n    \"three\",\n    \"four\",\n    \"five\",\n];\n",
            Action::Siblings,
            "let a = [\n    #[1|]#,\n    #(2|)#,\n    #(3|)#,\n    #(4|)#,\n    #(5|)#,\n];\n\nlet b = [\n    #(\"one\"|)#,\n    #(\"two\"|)#,\n    #(\"three\"|)#,\n    #(\"four\"|)#,\n    #(\"five\"|)#,\n];\n",
        ),
        (
            "let a = [\n    #[1|]#,\n    2,\n    #(3,\n    4|)#,\n    5,\n];\n",
            Action::Siblings,
            "let #(a|)# = #[[\n    1,\n    2,\n    3,\n    4,\n    5,\n]|]#;\n",
        ),
        (
            "let foo = bar(*#[a|]#, b, c);\n",
            Action::Siblings,
            "let foo = bar(#[*a|]#, #(b|)#, #(c|)#);\n",
        ),
        (
            "let foo = bar#[(a, b, c)|]#;\n",
            Action::Children,
            "let foo = bar(#[a|]#, #(b|)#, #(c|)#);\n",
        ),
        (
            "let a = #[[\n    1,\n    2,\n    3,\n    4,\n    5,\n]|]#;\n",
            Action::Children,
            "let a = [\n    #[1|]#,\n    #(2|)#,\n    #(3|)#,\n    #(4|)#,\n    #(5|)#,\n];\n",
        ),
        (
            "let a = #[|[\n    1,\n    2,\n    3,\n    4,\n    5,\n]]#;\n",
            Action::Children,
            "let a = [\n    #[|1]#,\n    #(|2)#,\n    #(|3)#,\n    #(|4)#,\n    #(|5)#,\n];\n",
        ),
        (
            "let a = [\n    #[1|]#,\n    #(2|)#,\n    #(3|)#,\n    #(4|)#,\n    #(5|)#,\n];\n",
            Action::Children,
            "let a = [\n    #[1|]#,\n    #(2|)#,\n    #(3|)#,\n    #(4|)#,\n    #(5|)#,\n];\n",
        ),
        (
            "let a = #[|[\n    1,\n    2,\n    3,\n    4,\n    5,\n]]#;\n\nlet b = #([\n    \"one\",\n    \"two\",\n    \"three\",\n    \"four\",\n    \"five\",\n]|)#;\n",
            Action::Children,
            "let a = [\n    #[|1]#,\n    #(|2)#,\n    #(|3)#,\n    #(|4)#,\n    #(|5)#,\n];\n\nlet b = [\n    #(\"one\"|)#,\n    #(\"two\"|)#,\n    #(\"three\"|)#,\n    #(\"four\"|)#,\n    #(\"five\"|)#,\n];\n",
        ),
        (
            "fn inc(x: usize) -> usize { x + 1 #[}|]#\nfn dec(x: usize) -> usize { x - 1 }\nfn ident(x: usize) -> usize { x }\n",
            Action::Next(false),
            "fn inc(x: usize) -> usize { x + 1 }\n#[fn dec(x: usize) -> usize { x - 1 }|]#\nfn ident(x: usize) -> usize { x }\n",
        ),
        (
            "fn inc(x: usize) -> usize { x + 1 #[}|]#\nfn dec(x: usize) -> usize { x - 1 }\nfn ident(x: usize) -> usize { x }\n",
            Action::Next(true),
            "fn inc(x: usize) -> usize { x + 1 }\nfn dec(x: usize) -> usize { x - 1 }\n#[fn ident(x: usize) -> usize { x }|]#\n",
        ),
        (
            "fn inc(x: usize) -> usize { x + 1 }\nfn dec(x: usize) -> usize { x - 1 }\n#[|f]#n ident(x: usize) -> usize { x }\n",
            Action::Previous(false),
            "fn inc(x: usize) -> usize { x + 1 }\n#[|fn dec(x: usize) -> usize { x - 1 }]#\nfn ident(x: usize) -> usize { x }\n",
        ),
        (
            "fn inc(x: usize) -> usize { x + 1 }\nfn dec(x: usize) -> usize { x - 1 }\n#[|f]#n ident(x: usize) -> usize { x }\n",
            Action::Previous(true),
            "#[|fn inc(x: usize) -> usize { x + 1 }]#\nfn dec(x: usize) -> usize { x - 1 }\nfn ident(x: usize) -> usize { x }\n",
        ),
    ];
    for (input, action, expected) in cases {
        let (text, mut selection) = test::print(input);
        let text = Rope::from(text);
        let syntax = Syntax::new(text.slice(..), language, &loader).unwrap();
        let slice = text.slice(..);
        match action {
            Action::Parent(direction, movement, count) => {
                for _ in 0..count {
                    selection = language::movement::move_parent_node_end(
                        &syntax, slice, selection, direction, movement,
                    );
                }
            }
            Action::Siblings => selection = object::select_all_siblings(&syntax, slice, selection),
            Action::Children => selection = object::select_all_children(&syntax, slice, selection),
            Action::Next(flip) | Action::Previous(flip) => {
                let next = matches!(action, Action::Next(_));
                let select = if next {
                    object::select_next_sibling
                } else {
                    object::select_prev_sibling
                };
                selection = select(&syntax, slice, selection);
                if flip {
                    selection = selection.transform(|range| range.flip());
                    selection = select(&syntax, slice, selection);
                }
            }
        }
        let (expected_text, expected_selection) = test::print(expected);
        assert_eq!(text, expected_text.as_str(), "{input}");
        assert_eq!(selection, expected_selection, "{input}");
    }
}
