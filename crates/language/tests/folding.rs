use std::{sync::LazyLock, time::Duration};

use editor_core::Rope;
use language::{syntax::Loader, Syntax};
use stdx::rope::RopeExt as _;

static LOADER: LazyLock<Loader> =
    LazyLock::new(|| language::loader::default_lang_loader(loader::syntax::Resources::default()));

// Exact spans ensure nested folds stop before the following sibling or section.
#[test]
fn additional_languages_fold_nested_regions() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "bash",
            "run() {\n  if true; then\n    echo café\n  fi\n}\necho after\n",
            &[
                "{\n  if true; then\n    echo café\n  fi\n}",
                "if true; then\n    echo café\n  fi",
            ],
        ),
        (
            "ruby",
            "class Example\n  def run\n    puts \"café\"\n  end\nend\nputs \"after\"\n",
            &[
                "class Example\n  def run\n    puts \"café\"\n  end\nend",
                "def run\n    puts \"café\"\n  end",
            ],
        ),
        (
            "java",
            "class Example {\n  void run() {\n    work();\n  }\n}\nclass After {}\n",
            &[
                "{\n  void run() {\n    work();\n  }\n}",
                "{\n    work();\n  }",
            ],
        ),
        (
            "c-sharp",
            "class Example {\n  void Run() {\n    Work();\n  }\n}\nclass After {}\n",
            &[
                "{\n  void Run() {\n    Work();\n  }\n}",
                "{\n    Work();\n  }",
            ],
        ),
        (
            "php",
            "<?php\nclass Example {\n  function run() {\n    work();\n  }\n}\necho \"after\";\n",
            &[
                "{\n  function run() {\n    work();\n  }\n}",
                "{\n    work();\n  }",
            ],
        ),
        (
            "swift",
            "class Example {\n  func run() {\n    work()\n  }\n}\nlet after = 1\n",
            &[
                "{\n  func run() {\n    work()\n  }\n}",
                "{\n    work()\n  }",
            ],
        ),
        (
            "scala",
            "object Example {\n  def run() = {\n    work()\n  }\n}\nval after = 1\n",
            &[
                "{\n  def run() = {\n    work()\n  }\n}",
                "{\n    work()\n  }",
            ],
        ),
        (
            "elixir",
            "defmodule Example do\n  def run do\n    :ok\n  end\nend\nafter_value = 1\n",
            &[
                "do\n  def run do\n    :ok\n  end\nend",
                "do\n    :ok\n  end",
            ],
        ),
        (
            "dart",
            "class Example {\n  void run() {\n    work();\n  }\n}\nvar after = 1;\n",
            &[
                "{\n  void run() {\n    work();\n  }\n}",
                "{\n    work();\n  }",
            ],
        ),
        (
            "css",
            "@media screen {\n  .example {\n    color: red;\n  }\n}\n.after {}\n",
            &[
                "{\n  .example {\n    color: red;\n  }\n}",
                "{\n    color: red;\n  }",
            ],
        ),
        (
            "scss",
            "@mixin example {\n  .nested {\n    color: red;\n  }\n}\n.after {}\n",
            &[
                "{\n  .nested {\n    color: red;\n  }\n}",
                "{\n    color: red;\n  }",
            ],
        ),
        (
            "yaml",
            "example:\n  values:\n    - café\n    - two\nafter: true\n",
            &[
                "example:\n  values:\n    - café\n    - two",
                "values:\n    - café\n    - two",
            ],
        ),
        (
            "toml",
            "[example]\nvalues = [\n  \"café\",\n  \"two\",\n]\n[after]\nenabled = true\n",
            &[
                "[example]\nvalues = [\n  \"café\",\n  \"two\",\n]\n",
                "[\n  \"café\",\n  \"two\",\n]",
            ],
        ),
        (
            "xml",
            "<example>\n  <nested>\n    café\n  </nested>\n</example>\n",
            &[
                "<example>\n  <nested>\n    café\n  </nested>\n</example>",
                "<nested>\n    café\n  </nested>",
            ],
        ),
        (
            "sql",
            "SELECT *\nFROM (\n  SELECT id\n  FROM example\n) AS nested;\nSELECT 1;\n",
            &[
                "SELECT *\nFROM (\n  SELECT id\n  FROM example\n) AS nested",
                "(\n  SELECT id\n  FROM example\n)",
            ],
        ),
        (
            "markdown",
            "# Example\n\nIntro café.\n\n## Nested\n\nText.\n\n# After\n\nOther.\n",
            &[
                "# Example\n\nIntro café.\n\n## Nested\n\nText.\n\n",
                "## Nested\n\nText.\n\n",
            ],
        ),
    ];
    assert_fold_regions(cases);
}

fn assert_fold_regions(cases: &[(&str, &str, &[&str])]) {
    for &(name, source, expected) in cases {
        let text = Rope::from_str(source);
        let language = LOADER.language_for_name(name).unwrap();
        LOADER.validate_queries(language).unwrap();
        let syntax =
            Syntax::new_with_timeout(text.slice(..), language, &LOADER, Duration::from_secs(10))
                .unwrap();
        let mut nodes = vec![syntax.tree().root_node()];
        while let Some(node) = nodes.pop() {
            assert!(
                node.kind() != "ERROR" && !node.is_missing(),
                "{name}: invalid fixture at {node:?}"
            );
            nodes.extend(node.children());
        }
        let folds = syntax.fold_ranges(text.slice(..), &LOADER, ..);
        let spans: Vec<_> = folds
            .iter()
            .map(|fold| text.slice_chars(fold.range.clone()).to_string())
            .collect();
        for &region in expected {
            let fold = folds
                .iter()
                .find(|fold| text.slice_chars(fold.range.clone()) == region)
                .unwrap_or_else(|| panic!("{name}: missing {region:?}; got {spans:?}"));
            // A Common Lisp defun's opener belongs to its header child.
            // Only direct paired tokens supply a closing delimiter.
            if name == "common-lisp" && region.starts_with("(defun ") {
                assert!(fold.closing_delimiter.is_none());
                continue;
            }
            if matches!(
                (region.chars().next(), region.chars().last()),
                (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
            ) {
                let closer = fold.closing_delimiter.expect(name);
                assert_eq!(text.get_char_at(closer), region.chars().last(), "{name}");
            }
        }
    }
}

#[test]
fn additional_languages_fold_comments_collections_and_text_blocks() {
    assert_fold_regions(&[
        (
            "java",
            "/* café\n * details\n */\nclass After {}",
            &["/* café\n * details\n */"],
        ),
        (
            "scss",
            "/* café\n * details\n */\n.after {}",
            &["/* café\n * details\n */"],
        ),
        (
            "ruby",
            "values = [\n  1,\n  2\n]\nputs :after",
            &["[\n  1,\n  2\n]"],
        ),
        (
            "yaml",
            "items:\n  - name: café\n    enabled: true\n  - name: after\n",
            &["- name: café\n    enabled: true"],
        ),
        (
            "yaml",
            "message: |\n  café\n  details\nafter: true\n",
            &["|\n  café\n  details"],
        ),
        (
            "toml",
            "[[example]]\nname = \"café\"\n[[example]]\nname = \"after\"\n",
            &["[[example]]\nname = \"café\"\n"],
        ),
        (
            "markdown",
            "Intro.\n\n```text\ncafé\ndetails\n```\n\nAfter.\n",
            &["```text\ncafé\ndetails\n```\n"],
        ),
    ]);
}

#[test]
fn markdown_does_not_fold_headingless_sections_across_setext_headings() {
    let text = Rope::from_str("Example\n=======\n\ncafé\n\nAfter\n=====\n\nOther.\n");
    let language = LOADER.language_for_name("markdown").unwrap();
    let syntax = Syntax::new(text.slice(..), language, &LOADER).unwrap();
    assert!(syntax.fold_ranges(text.slice(..), &LOADER, ..).is_empty());
}

macro_rules! fold_case {
    ($test:ident, $language:literal, $source:literal, [$($region:literal),+ $(,)?]) => {
        #[test]
        fn $test() {
            assert_fold_regions(&[($language, $source, &[$($region),+])]);
        }
    };
}

fold_case!(
    zig_folds,
    "zig",
    "fn run() void {\n    if (true) {\n        work();\n    }\n}\nconst after = 1;\n",
    [
        "{\n    if (true) {\n        work();\n    }\n}",
        "{\n        work();\n    }"
    ]
);

fold_case!(
    r_folds,
    "r",
    "run <- function() {\n  if (TRUE) {\n    print(\"café\")\n  }\n}\nafter <- 1\n",
    [
        "{\n  if (TRUE) {\n    print(\"café\")\n  }\n}",
        "{\n    print(\"café\")\n  }"
    ]
);

fold_case!(
    haskell_folds,
    "haskell",
    "run x = do\n  print x\n  pure x\nafter = 1\n",
    ["run x = do\n  print x\n  pure x", "do\n  print x\n  pure x"]
);

fold_case!(
    ocaml_folds,
    "ocaml",
    "module Example = struct\n  let values = [\n    1;\n    2\n  ]\nend\nlet after = 1\n",
    [
        "struct\n  let values = [\n    1;\n    2\n  ]\nend",
        "[\n    1;\n    2\n  ]"
    ]
);

fold_case!(
    ocaml_interface_folds,
    "ocaml-interface",
    "module Example : sig\n  val run :\n    int -> int\nend\nval after : int\n",
    ["sig\n  val run :\n    int -> int\nend"]
);

fold_case!(
    clojure_folds,
    "clojure",
    "(defn run []\n  (let [value 1]\n    value))\n(def after 1)\n",
    [
        "(defn run []\n  (let [value 1]\n    value))",
        "(let [value 1]\n    value)"
    ]
);

fold_case!(
    scheme_folds,
    "scheme",
    "(define (run)\n  (let ((value 1))\n    value))\n(define after 1)\n",
    [
        "(define (run)\n  (let ((value 1))\n    value))",
        "(let ((value 1))\n    value)"
    ]
);

fold_case!(
    elisp_folds,
    "elisp",
    "(defun run ()\n  (let ((value 1))\n    value))\n(setq after 1)\n",
    [
        "(defun run ()\n  (let ((value 1))\n    value))",
        "(let ((value 1))\n    value)"
    ]
);

fold_case!(
    common_lisp_folds,
    "common-lisp",
    "(defun run ()\n  (let ((value 1))\n    value))\n(defvar after 1)\n",
    [
        "(defun run ()\n  (let ((value 1))\n    value))",
        "(let ((value 1))\n    value)"
    ]
);

fold_case!(
    erlang_folds,
    "erlang",
    "run(X) ->\n  case X of\n    ok -> true;\n    _ -> false\n  end.\nafter_value() -> ok.\n",
    [
        "run(X) ->\n  case X of\n    ok -> true;\n    _ -> false\n  end",
        "case X of\n    ok -> true;\n    _ -> false\n  end"
    ]
);

fold_case!(
    elm_folds,
    "elm",
    "values =\n    [ 1\n    , 2\n    ]\n\nafter = 1\n",
    ["values =\n    [ 1\n    , 2\n    ]", "[ 1\n    , 2\n    ]"]
);

fold_case!(vue_folds, "vue", "<template>\n  <div>\n    <span>café</span>\n  </div>\n</template>\n<script>\nfunction run() {\n  work();\n}\n</script>\n", ["<template>\n  <div>\n    <span>café</span>\n  </div>\n</template>", "<div>\n    <span>café</span>\n  </div>", "{\n  work();\n}"]);

fold_case!(
    astro_folds,
    "astro",
    "---\nconst values = [\n  1,\n  2\n];\n---\n<div>\n  <span>café</span>\n</div>\n",
    [
        "---\nconst values = [\n  1,\n  2\n];\n---",
        "[\n  1,\n  2\n]",
        "<div>\n  <span>café</span>\n</div>"
    ]
);

fold_case!(cmake_folds, "cmake", "function(example)\n  if(ENABLED)\n    message(STATUS \"café\")\n  endif()\nendfunction()\nmessage(STATUS \"after\")\n", ["function(example)\n  if(ENABLED)\n    message(STATUS \"café\")\n  endif()\nendfunction()", "if(ENABLED)\n    message(STATUS \"café\")\n  endif()"]);

fold_case!(
    meson_folds,
    "meson",
    "if enabled\n  values = [\n    'café',\n    'two',\n  ]\nendif\nafter = true\n",
    [
        "if enabled\n  values = [\n    'café',\n    'two',\n  ]\nendif",
        "[\n    'café',\n    'two',\n  ]"
    ]
);

fold_case!(protobuf_folds, "protobuf", "syntax = \"proto3\";\nmessage Example {\n  message Nested {\n    string name = 1;\n  }\n}\nmessage After {}\n", ["{\n  message Nested {\n    string name = 1;\n  }\n}", "{\n    string name = 1;\n  }"]);

fold_case!(
    graphql_folds,
    "graphql",
    "query Example {\n  viewer {\n    id\n  }\n}\nquery After { id }\n",
    ["{\n  viewer {\n    id\n  }\n}", "{\n    id\n  }"]
);

fold_case!(
    fish_folds,
    "fish",
    "function run\n  if true\n    echo café\n  end\nend\necho after\n",
    [
        "function run\n  if true\n    echo café\n  end\nend",
        "if true\n    echo café\n  end"
    ]
);

fold_case!(
    powershell_folds,
    "powershell",
    "function Run {\n  if ($true) {\n    Write-Output 'café'\n  }\n}\nWrite-Output 'after'\n",
    [
        "function Run {\n  if ($true) {\n    Write-Output 'café'\n  }\n}",
        "{\n    Write-Output 'café'\n  }"
    ]
);

fold_case!(
    dockerfile_folds,
    "dockerfile",
    "FROM alpine\nCMD [\\\n  \"echo\",\\\n  \"café\"\\\n]\nENV AFTER=true\n",
    [
        "CMD [\\\n  \"echo\",\\\n  \"café\"\\\n]",
        "[\\\n  \"echo\",\\\n  \"café\"\\\n]"
    ]
);

fold_case!(
    make_folds,
    "make",
    "example:\n\techo café\n\techo two\n\nafter:\n\techo after\n",
    ["example:\n\techo café\n\techo two\n\n"]
);

#[test]
fn language_variants_inherit_folding() {
    assert_fold_regions(&[
        (
            "jsonc",
            "{\n  \"values\": [\n    1\n  ]\n}\n",
            &["[\n    1\n  ]"],
        ),
        (
            "json-ld",
            "{\n  \"values\": [\n    1\n  ]\n}\n",
            &["[\n    1\n  ]"],
        ),
        (
            "ansible",
            "example:\n  name: café\n  enabled: true\nafter: true\n",
            &["example:\n  name: café\n  enabled: true"],
        ),
        (
            "nestedtext",
            "example:\n  name: café\n  enabled: true\nafter: true\n",
            &["example:\n  name: café\n  enabled: true"],
        ),
        (
            "mdx",
            "# Example\n\ncafé\n\n# After\n\nText.\n",
            &["# Example\n\ncafé\n\n"],
        ),
        (
            "markdown-rustdoc",
            "# Example\n\ncafé\n\n# After\n\nText.\n",
            &["# Example\n\ncafé\n\n"],
        ),
        (
            "docker-compose",
            "example:\n  name: café\n  enabled: true\nafter: true\n",
            &["example:\n  name: café\n  enabled: true"],
        ),
        (
            "github-action",
            "example:\n  name: café\n  enabled: true\nafter: true\n",
            &["example:\n  name: café\n  enabled: true"],
        ),
        (
            "quarto",
            "# Example\n\ncafé\n\n# After\n\nText.\n",
            &["# Example\n\ncafé\n\n"],
        ),
        (
            "rmarkdown",
            "# Example\n\ncafé\n\n# After\n\nText.\n",
            &["# Example\n\ncafé\n\n"],
        ),
        (
            "starlark",
            "def run():\n    return \"café\"\n\nafter = True\n",
            &["def run():\n    return \"café\""],
        ),
        (
            "sage",
            "def run():\n    return \"café\"\n\nafter = True\n",
            &["def run():\n    return \"café\""],
        ),
        (
            "msbuild",
            "<example>\n  café\n</example>\n",
            &["<example>\n  café\n</example>"],
        ),
        (
            "webc",
            "<div>\n  café\n</div>\n",
            &["<div>\n  café\n</div>"],
        ),
        (
            "jjconfig",
            "[example]\nname = \"café\"\n[after]\nenabled = true\n",
            &["[example]\nname = \"café\"\n"],
        ),
        (
            "miseconfig",
            "[example]\nname = \"café\"\n[after]\nenabled = true\n",
            &["[example]\nname = \"café\"\n"],
        ),
        (
            "pkgbuild",
            "run() {\n  echo café\n}\necho after\n",
            &["{\n  echo café\n}"],
        ),
        (
            "cross-config",
            "[example]\nname = \"café\"\n[after]\nenabled = true\n",
            &["[example]\nname = \"café\"\n"],
        ),
        (
            "git-cliff-config",
            "[example]\nname = \"café\"\n[after]\nenabled = true\n",
            &["[example]\nname = \"café\"\n"],
        ),
        (
            "gitlab-ci",
            "example:\n  name: café\n  enabled: true\nafter: true\n",
            &["example:\n  name: café\n  enabled: true"],
        ),
        (
            "woodpecker-ci",
            "example:\n  name: café\n  enabled: true\nafter: true\n",
            &["example:\n  name: café\n  enabled: true"],
        ),
        (
            "tilt",
            "def run():\n    return \"café\"\n\nafter = True\n",
            &["def run():\n    return \"café\""],
        ),
    ]);
}
