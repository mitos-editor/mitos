//! Language definitions and settings, independent of parser and editor runtime state.

mod spelling;
pub use spelling::{ParseSpellingLanguageError, SpellingConfig, SpellingLanguage};

use ::diagnostics::Severity;
use editor_core::auto_pairs::AutoPairs;
use editor_core::comment::BlockCommentToken;

mod debugger;
pub use debugger::{
    AdvancedCompletion, DebugAdapterConfig, DebugAdapterTransport, DebugConfigCompletion,
    DebugTemplate, DebuggerQuirks,
};
use serde::{ser::SerializeSeq as _, Deserialize, Serialize};
use stdx::rope;

use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display},
    num::NonZeroU8,
    path::PathBuf,
    str::FromStr,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Configuration {
    pub language: Vec<LanguageConfiguration>,
    #[serde(default)]
    pub language_server: HashMap<String, LanguageServerConfiguration>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LanguageConfiguration {
    #[serde(rename = "name")]
    pub language_id: String, // c-sharp, rust, tsx
    #[serde(rename = "language-id")]
    // see the table under https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocumentItem
    pub language_server_language_id: Option<String>, // csharp, rust, typescriptreact, for the language-server
    pub scope: String,             // source.rust
    pub file_types: Vec<FileType>, // filename extension or ends_with? <Gemfile, rb, etc>
    #[serde(default)]
    pub shebangs: Vec<String>, // interpreter(s) associated with language
    #[serde(default)]
    pub roots: RootMarkers, // these indicate project roots <.git, Cargo.toml>
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "from_comment_tokens",
        alias = "comment-token"
    )]
    pub comment_tokens: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing,
        deserialize_with = "from_block_comment_tokens"
    )]
    pub block_comment_tokens: Option<Vec<BlockCommentToken>>,
    pub text_width: Option<usize>,
    pub soft_wrap: Option<SoftWrap>,

    pub auto_format: Option<bool>,

    #[serde(default)]
    pub code_actions_on_save: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub formatter: Option<FormatterConfiguration>,

    /// If set, overrides `editor.completion.path.enable`.
    pub path_completion: Option<bool>,
    /// If set, overrides `editor.completion.word`.
    pub word_completion: Option<WordCompletion>,

    /// Layers over `editor.spelling` for this language.
    pub spelling: Option<SpellingConfig>,

    #[serde(default)]
    pub diagnostic_severity: Severity,

    pub grammar: Option<String>, // tree-sitter grammar name, defaults to language_id

    // content_regex
    #[serde(default, skip_serializing, deserialize_with = "deserialize_regex")]
    pub injection_regex: Option<rope::Regex>,
    // first_line_regex
    //
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_lang_features",
        deserialize_with = "deserialize_lang_features"
    )]
    pub language_servers: Vec<LanguageServerFeatures>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indent: Option<IndentationConfiguration>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub debugger: Option<DebugAdapterConfig>,

    /// Automatic insertion of pairs to parentheses, brackets,
    /// etc. A boolean uses or disables the built-in pairs; a character map
    /// supplies language-specific pairs. Global disablement takes precedence.
    #[serde(default, skip_serializing, deserialize_with = "deserialize_auto_pairs")]
    pub auto_pairs: Option<AutoPairs>,

    pub rulers: Option<Vec<u16>>, // if set, override editor's rulers

    /// Hardcoded LSP root directories relative to the workspace root, like `examples` or `tools/fuzz`.
    /// Falling back to the current working directory if none are configured.
    pub workspace_lsp_roots: Option<Vec<PathBuf>>,
    #[serde(default)]
    pub persistent_diagnostic_sources: Vec<String>,
    /// Overrides `editor.rainbow-brackets.enable` for the language.
    pub rainbow_brackets: Option<bool>,
}

impl LanguageConfiguration {
    /// Single servers and contiguous groups of ordered alternatives.
    pub fn language_server_groups(&self) -> impl Iterator<Item = &[LanguageServerFeatures]> {
        self.language_servers.chunk_by(|a, b| {
            a.alternative_group.is_some() && a.alternative_group == b.alternative_group
        })
    }
}

pub type RootMarkers = GlobSet;

/// A wrapper around `globset::GlobSet` which implements `Serialize` and `Deserialize`.
#[derive(Default, Debug)]
pub struct GlobSet {
    inner: globset::GlobSet,
    /// Glob patterns as-is before building. This is only used for `Serialize`.
    patterns: Vec<String>,
}

impl GlobSet {
    pub fn is_match<P: AsRef<std::path::Path>>(&self, path: P) -> bool {
        self.inner.is_match(path)
    }
}

impl Serialize for GlobSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut patterns = serializer.serialize_seq(Some(self.patterns.len()))?;
        for pattern in &self.patterns {
            patterns.serialize_element(pattern)?;
        }
        patterns.end()
    }
}

impl<'de> Deserialize<'de> for GlobSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let patterns: Vec<String> = Deserialize::deserialize(deserializer)?;
        let mut builder = globset::GlobSetBuilder::new();
        for pattern in &patterns {
            let glob = globset::Glob::new(pattern).map_err(serde::de::Error::custom)?;
            builder.add(glob);
        }
        let inner = builder.build().map_err(serde::de::Error::custom)?;
        Ok(Self { inner, patterns })
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// The extension of the file, either the `Path::extension` or the full
    /// filename if the file does not have an extension.
    Extension(String),
    /// A Unix-style path glob. This is compared to the file's absolute path, so
    /// it can be used to detect files based on their directories. If the glob
    /// is not an absolute path and does not already start with a glob pattern,
    /// a glob pattern will be prepended to it.
    Glob(globset::Glob),
}

impl Serialize for FileType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        match self {
            FileType::Extension(extension) => serializer.serialize_str(extension),
            FileType::Glob(glob) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("glob", glob.glob())?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for FileType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        struct FileTypeVisitor;

        impl<'de> serde::de::Visitor<'de> for FileTypeVisitor {
            type Value = FileType;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("string or table")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FileType::Extension(value.to_string()))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                match map.next_entry::<String, String>()? {
                    Some((key, mut glob)) if key == "glob" => {
                        // If the glob isn't an absolute path or already starts
                        // with a glob pattern, add a leading glob so we
                        // properly match relative paths.
                        if !glob.starts_with('/') && !glob.starts_with("*/") {
                            glob.insert_str(0, "*/");
                        }

                        globset::Glob::new(glob.as_str())
                            .map(FileType::Glob)
                            .map_err(|err| {
                                serde::de::Error::custom(format!("invalid `glob` pattern: {}", err))
                            })
                    }
                    Some((key, _value)) => Err(serde::de::Error::custom(format!(
                        "unknown key in `file-types` list: {}",
                        key
                    ))),
                    None => Err(serde::de::Error::custom(
                        "expected a `suffix` key in the `file-types` entry",
                    )),
                }
            }
        }

        deserializer.deserialize_any(FileTypeVisitor)
    }
}

fn from_comment_tokens<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CommentTokens {
        Multiple(Vec<String>),
        Single(String),
    }
    Ok(
        Option::<CommentTokens>::deserialize(deserializer)?.map(|tokens| match tokens {
            CommentTokens::Single(val) => vec![val],
            CommentTokens::Multiple(vals) => vals,
        }),
    )
}

fn from_block_comment_tokens<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<BlockCommentToken>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BlockCommentTokens {
        Multiple(Vec<BlockCommentToken>),
        Single(BlockCommentToken),
    }
    Ok(
        Option::<BlockCommentTokens>::deserialize(deserializer)?.map(|tokens| match tokens {
            BlockCommentTokens::Single(val) => vec![val],
            BlockCommentTokens::Multiple(vals) => vals,
        }),
    )
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum LanguageServerFeature {
    Format,
    GotoDeclaration,
    GotoDefinition,
    GotoTypeDefinition,
    GotoReference,
    GotoImplementation,
    // Goto, use bitflags, combining previous Goto members?
    SignatureHelp,
    Hover,
    DocumentHighlight,
    Completion,
    CodeAction,
    DocumentLinks,
    WorkspaceCommand,
    DocumentSymbols,
    WorkspaceSymbols,
    // Symbols, use bitflags, see above?
    Diagnostics,
    PullDiagnostics,
    RenameSymbol,
    InlayHints,
    InlineValues,
    DocumentColors,
    InlineCompletion,
    CallHierarchy,
    CodeLens,
}

impl Display for LanguageServerFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use LanguageServerFeature::*;
        let feature = match self {
            Format => "format",
            GotoDeclaration => "goto-declaration",
            GotoDefinition => "goto-definition",
            GotoTypeDefinition => "goto-type-definition",
            GotoReference => "goto-reference",
            GotoImplementation => "goto-implementation",
            SignatureHelp => "signature-help",
            Hover => "hover",
            DocumentHighlight => "document-highlight",
            Completion => "completion",
            CodeAction => "code-action",
            DocumentLinks => "document-links",
            WorkspaceCommand => "workspace-command",
            DocumentSymbols => "document-symbols",
            WorkspaceSymbols => "workspace-symbols",
            Diagnostics => "diagnostics",
            PullDiagnostics => "pull-diagnostics",
            RenameSymbol => "rename-symbol",
            InlayHints => "inlay-hints",
            InlineValues => "inline-values",
            DocumentColors => "document-colors",
            InlineCompletion => "inline-completion",
            CallHierarchy => "call-hierarchy",
            CodeLens => "code-lens",
        };
        write!(f, "{feature}",)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged, rename_all = "kebab-case", deny_unknown_fields)]
enum LanguageServerFeatureConfiguration {
    #[serde(rename_all = "kebab-case")]
    Features {
        #[serde(default, skip_serializing_if = "HashSet::is_empty")]
        only_features: HashSet<LanguageServerFeature>,
        #[serde(default, skip_serializing_if = "HashSet::is_empty")]
        except_features: HashSet<LanguageServerFeature>,
        name: String,
    },
    Simple(String),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum LanguageServerEntry {
    Server(LanguageServerFeatureConfiguration),
    Alternatives {
        alternatives: Vec<LanguageServerFeatureConfiguration>,
    },
}

#[derive(Debug, Default)]
pub struct LanguageServerFeatures {
    pub name: String,
    pub only: HashSet<LanguageServerFeature>,
    pub excluded: HashSet<LanguageServerFeature>,
    /// Entries from an alternatives table share its position in the original list.
    /// Flattening preserves feature routing and the configured server order.
    pub alternative_group: Option<usize>,
}

impl LanguageServerFeatures {
    pub fn has_feature(&self, feature: LanguageServerFeature) -> bool {
        (self.only.is_empty() || self.only.contains(&feature)) && !self.excluded.contains(&feature)
    }
}

impl From<LanguageServerFeatureConfiguration> for LanguageServerFeatures {
    fn from(config: LanguageServerFeatureConfiguration) -> Self {
        match config {
            LanguageServerFeatureConfiguration::Simple(name) => Self {
                name,
                ..Default::default()
            },
            LanguageServerFeatureConfiguration::Features {
                only_features,
                except_features,
                name,
            } => Self {
                name,
                only: only_features,
                excluded: except_features,
                ..Default::default()
            },
        }
    }
}

impl From<&LanguageServerFeatures> for LanguageServerFeatureConfiguration {
    fn from(features: &LanguageServerFeatures) -> Self {
        if features.only.is_empty() && features.excluded.is_empty() {
            Self::Simple(features.name.clone())
        } else {
            Self::Features {
                only_features: features.only.clone(),
                except_features: features.excluded.clone(),
                name: features.name.clone(),
            }
        }
    }
}

fn deserialize_lang_features<'de, D>(
    deserializer: D,
) -> Result<Vec<LanguageServerFeatures>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<LanguageServerEntry> = Deserialize::deserialize(deserializer)?;
    let mut result = Vec::new();
    for (index, entry) in raw.into_iter().enumerate() {
        match entry {
            LanguageServerEntry::Server(server) => result.push(server.into()),
            LanguageServerEntry::Alternatives { alternatives } => {
                if alternatives.is_empty() {
                    return Err(serde::de::Error::custom(
                        "language-server alternatives must not be empty",
                    ));
                }
                for alternative in alternatives {
                    let mut features = LanguageServerFeatures::from(alternative);
                    features.alternative_group = Some(index);
                    result.push(features);
                }
            }
        }
    }
    let mut names = HashMap::new();
    for server in &result {
        if let Some(previous_group) = names.insert(&server.name, server.alternative_group)
            && (previous_group.is_some() || server.alternative_group.is_some())
        {
            return Err(serde::de::Error::custom(format!(
                "language server '{}' occurs more than once in an alternatives configuration",
                server.name
            )));
        }
    }
    Ok(result)
}

fn serialize_lang_features<S>(
    servers: &[LanguageServerFeatures],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut serializer = serializer.serialize_seq(None)?;
    for group in servers.chunk_by(|a, b| {
        a.alternative_group.is_some() && a.alternative_group == b.alternative_group
    }) {
        let entry = if group[0].alternative_group.is_some() {
            LanguageServerEntry::Alternatives {
                alternatives: group.iter().map(Into::into).collect(),
            }
        } else {
            LanguageServerEntry::Server((&group[0]).into())
        };
        serializer.serialize_element(&entry)?;
    }
    serializer.end()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LanguageServerConfiguration {
    pub command: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub environment: HashMap<String, String>,
    #[serde(default, skip_serializing, deserialize_with = "deserialize_lsp_config")]
    pub config: Option<serde_json::Value>,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub required_root_patterns: Option<GlobSet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FormatterConfiguration {
    pub command: String,
    /// Maximum formatter runtime in seconds.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IndentationConfiguration {
    #[serde(deserialize_with = "deserialize_tab_width")]
    pub tab_width: usize,
    pub unit: String,
}

/// How the indentation for a newly inserted line should be determined.
/// If the selected heuristic is not available (e.g. because the current
/// language has no tree-sitter indent queries), a simpler one will be used.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndentationHeuristic {
    /// Copy the current line's leading whitespace character for character.
    Verbatim,
    /// Match the current line's indentation width using the selected indentation style.
    Simple,
    /// Use tree-sitter indent queries to compute the expected absolute indentation level of the new line.
    TreeSitter,
    /// Use tree-sitter indent queries to compute the expected difference in indentation between the new line
    /// and the line before. Add this to the actual indentation level of the line before.
    #[default]
    Hybrid,
}

/// Automatic delimiter pairing, shared by global and language configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct AutoPairConfig {
    pub enable: bool,
    /// Omitted uses the built-in pairs; an empty map disables all pairs.
    pub pairs: Option<HashMap<char, char>>,
}

impl Default for AutoPairConfig {
    fn default() -> Self {
        Self {
            enable: true,
            pairs: None,
        }
    }
}

impl From<&AutoPairConfig> for Option<AutoPairs> {
    fn from(config: &AutoPairConfig) -> Self {
        config.enable.then(|| match &config.pairs {
            Some(pairs) => AutoPairs::new(pairs.iter()),
            None => AutoPairs::default(),
        })
    }
}

impl From<AutoPairConfig> for Option<AutoPairs> {
    fn from(config: AutoPairConfig) -> Self {
        (&config).into()
    }
}

impl FromStr for AutoPairConfig {
    type Err = std::str::ParseBoolError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            enable: s.parse()?,
            ..Self::default()
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct SoftWrap {
    /// Soft wrap lines that exceed viewport width. Default to off
    // NOTE: Option on purpose because the struct is shared between language config and global config.
    // By default the option is None so that the language config falls back to the global config unless explicitly set.
    pub enable: Option<bool>,
    /// Maximum space left free at the end of the line.
    /// This space is used to wrap text at word boundaries. If that is not possible within this limit
    /// the word is simply split at the end of the line.
    ///
    /// This is automatically hard-limited to a quarter of the viewport to ensure correct display on small views.
    ///
    /// Default to 20
    pub max_wrap: Option<u16>,
    /// Maximum number of indentation that can be carried over from the previous line when softwrapping.
    /// If a line is indented further then this limit it is rendered at the start of the viewport instead.
    ///
    /// This is automatically hard-limited to a quarter of the viewport to ensure correct display on small views.
    ///
    /// Default to 40
    pub max_indent_retain: Option<u16>,
    /// Indicator placed at the beginning of softwrapped lines
    ///
    /// Defaults to ↪
    pub wrap_indicator: Option<String>,
    /// Softwrap at `text_width` instead of viewport width if it is shorter
    pub wrap_at_text_width: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct WordCompletion {
    pub enable: Option<bool>,
    pub trigger_length: Option<NonZeroU8>,
}

fn deserialize_regex<'de, D>(deserializer: D) -> Result<Option<rope::Regex>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|buf| rope::Regex::new(&buf).map_err(serde::de::Error::custom))
        .transpose()
}

fn deserialize_lsp_config<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<toml::Value>::deserialize(deserializer)?
        .map(|toml| toml.try_into().map_err(serde::de::Error::custom))
        .transpose()
}

fn deserialize_tab_width<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    usize::deserialize(deserializer).and_then(|n| {
        if n > 0 && n <= 16 {
            Ok(n)
        } else {
            Err(serde::de::Error::custom(
                "tab width must be a value from 1 to 16 inclusive",
            ))
        }
    })
}

pub fn deserialize_auto_pairs<'de, D>(deserializer: D) -> Result<Option<AutoPairs>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum LanguageAutoPairs {
        Enable(bool),
        Pairs(HashMap<char, char>),
    }
    Ok(
        Option::<LanguageAutoPairs>::deserialize(deserializer)?.and_then(|config| match config {
            LanguageAutoPairs::Enable(false) => None,
            LanguageAutoPairs::Enable(true) => Some(AutoPairs::default()),
            LanguageAutoPairs::Pairs(pairs) => Some(AutoPairs::new(pairs.iter())),
        }),
    )
}

fn default_timeout() -> u64 {
    20
}
