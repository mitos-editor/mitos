//! Standard language identifiers, including custom values (LSP 3.18).

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// A language identifier. Unknown language identifiers are preserved.
#[derive(Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Clone, Deserialize, Serialize)]
pub struct LanguageKind(Cow<'static, str>);

impl LanguageKind {
    pub const ABAP: Self = Self::new("abap");
    pub const WINDOWS_BAT: Self = Self::new("bat");
    pub const BIBTEX: Self = Self::new("bibtex");
    pub const CLOJURE: Self = Self::new("clojure");
    pub const COFFEESCRIPT: Self = Self::new("coffeescript");
    pub const C: Self = Self::new("c");
    pub const CPP: Self = Self::new("cpp");
    pub const C_SHARP: Self = Self::new("csharp");
    pub const CSS: Self = Self::new("css");
    pub const D: Self = Self::new("d");
    pub const DELPHI: Self = Self::new("pascal");
    pub const DIFF: Self = Self::new("diff");
    pub const DART: Self = Self::new("dart");
    pub const DOCKERFILE: Self = Self::new("dockerfile");
    pub const ELIXIR: Self = Self::new("elixir");
    pub const ERLANG: Self = Self::new("erlang");
    pub const F_SHARP: Self = Self::new("fsharp");
    pub const GIT_COMMIT: Self = Self::new("git-commit");
    pub const GIT_REBASE: Self = Self::new("git-rebase");
    pub const GO: Self = Self::new("go");
    pub const GROOVY: Self = Self::new("groovy");
    pub const HANDLEBARS: Self = Self::new("handlebars");
    pub const HASKELL: Self = Self::new("haskell");
    pub const HTML: Self = Self::new("html");
    pub const INI: Self = Self::new("ini");
    pub const JAVA: Self = Self::new("java");
    pub const JAVASCRIPT: Self = Self::new("javascript");
    pub const JAVASCRIPT_REACT: Self = Self::new("javascriptreact");
    pub const JSON: Self = Self::new("json");
    pub const LATEX: Self = Self::new("latex");
    pub const LESS: Self = Self::new("less");
    pub const LUA: Self = Self::new("lua");
    pub const MAKEFILE: Self = Self::new("makefile");
    pub const MARKDOWN: Self = Self::new("markdown");
    pub const OBJECTIVE_C: Self = Self::new("objective-c");
    pub const OBJECTIVE_CPP: Self = Self::new("objective-cpp");
    pub const PASCAL: Self = Self::new("pascal");
    pub const PERL: Self = Self::new("perl");
    pub const PERL6: Self = Self::new("perl6");
    pub const PHP: Self = Self::new("php");
    pub const PLAINTEXT: Self = Self::new("plaintext");
    pub const POWERSHELL: Self = Self::new("powershell");
    pub const PUG: Self = Self::new("jade");
    pub const PYTHON: Self = Self::new("python");
    pub const R: Self = Self::new("r");
    pub const RAZOR: Self = Self::new("razor");
    pub const RUBY: Self = Self::new("ruby");
    pub const RUST: Self = Self::new("rust");
    pub const SCSS: Self = Self::new("scss");
    pub const SASS: Self = Self::new("sass");
    pub const SCALA: Self = Self::new("scala");
    pub const SHADER_LAB: Self = Self::new("shaderlab");
    pub const SHELL_SCRIPT: Self = Self::new("shellscript");
    pub const SQL: Self = Self::new("sql");
    pub const SWIFT: Self = Self::new("swift");
    pub const TYPESCRIPT: Self = Self::new("typescript");
    pub const TYPESCRIPT_REACT: Self = Self::new("typescriptreact");
    pub const TEX: Self = Self::new("tex");
    pub const VISUAL_BASIC: Self = Self::new("vb");
    pub const XML: Self = Self::new("xml");
    pub const XSL: Self = Self::new("xsl");
    pub const YAML: Self = Self::new("yaml");

    pub const fn new(value: &'static str) -> Self {
        Self(Cow::Borrowed(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for LanguageKind {
    fn from(value: String) -> Self {
        Self(Cow::Owned(value))
    }
}

impl From<&'static str> for LanguageKind {
    fn from(value: &'static str) -> Self {
        Self::new(value)
    }
}
