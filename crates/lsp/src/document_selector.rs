//! Compiled selectors from static provider registrations, matched against document URI paths.

use std::{collections::HashMap, sync::LazyLock};

use anyhow::Context as _;
use globset::GlobBuilder;
use regex::{Regex, RegexBuilder};

use crate::lsp;

#[derive(Debug, Default)]
pub(crate) struct DocumentSelectors(HashMap<&'static str, Vec<Filter>>);

#[derive(Debug)]
struct Filter {
    language: Option<String>,
    scheme: Option<String>,
    pattern: Option<Pattern>,
}

#[derive(Debug)]
struct Pattern {
    base: Option<(lsp::Uri, String)>,
    matcher: Regex,
}

impl DocumentSelectors {
    pub(crate) fn new(capabilities: &lsp::ServerCapabilities) -> Self {
        use lsp::*;
        let mut selectors = Self::default();
        if let Some(DeclarationCapability::RegistrationOptions(options)) =
            &capabilities.declaration_provider
        {
            selectors.insert::<request::GotoDeclaration>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(TypeDefinitionProviderCapability::RegistrationOptions(options)) =
            &capabilities.type_definition_provider
        {
            selectors.insert::<request::GotoTypeDefinition>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(ImplementationProviderCapability::RegistrationOptions(options)) =
            &capabilities.implementation_provider
        {
            selectors.insert::<request::GotoImplementation>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(ColorProviderCapability::Options(options)) = &capabilities.color_provider {
            selectors.insert::<request::DocumentColor>(&options.document_selector);
        }
        if let Some(FoldingRangeProviderCapability::Options(options)) =
            &capabilities.folding_range_provider
        {
            selectors.insert::<request::FoldingRangeRequest>(&options.document_selector);
        }
        if let Some(SelectionRangeProviderCapability::RegistrationOptions(options)) =
            &capabilities.selection_range_provider
        {
            selectors.insert::<request::SelectionRangeRequest>(
                &options.registration_options.document_selector,
            );
        }
        if let Some(TypeHierarchyServerCapabilities::RegistrationOptions(options)) =
            &capabilities.type_hierarchy_provider
        {
            selectors.insert::<request::TypeHierarchyPrepare>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(CallHierarchyServerCapability::RegistrationOptions(options)) =
            &capabilities.call_hierarchy_provider
        {
            selectors.insert::<request::CallHierarchyPrepare>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(options)) =
            &capabilities.semantic_tokens_provider
        {
            selectors.insert::<request::SemanticTokensFullRequest>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(OneOf::Right(InlayHintServerCapabilities::RegistrationOptions(options))) =
            &capabilities.inlay_hint_provider
        {
            selectors.insert::<request::InlayHintRequest>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(OneOf::Right(InlineValueServerCapabilities::RegistrationOptions(options))) =
            &capabilities.inline_value_provider
        {
            selectors.insert::<request::InlineValueRequest>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(LinkedEditingRangeServerCapabilities::RegistrationOptions(options)) =
            &capabilities.linked_editing_range_provider
        {
            selectors.insert::<request::LinkedEditingRange>(
                &options.text_document_registration_options.document_selector,
            );
        }
        if let Some(DiagnosticServerCapabilities::RegistrationOptions(options)) =
            &capabilities.diagnostic_provider
        {
            selectors.insert::<request::DocumentDiagnosticRequest>(
                &options.text_document_registration_options.document_selector,
            );
        }
        selectors
    }

    fn insert<R: lsp::request::Request>(&mut self, selector: &Option<lsp::DocumentSelector>) {
        // Null uses the document binding chosen by the client; [] selects no documents.
        let Some(selector) = selector else { return };
        let filters = selector
            .iter()
            .filter_map(|filter| {
                let lsp::DocumentFilter::Text(filter) = filter else {
                    return None;
                };
                match Filter::new(filter) {
                    Ok(filter) => Some(filter),
                    Err(error) => {
                        log::warn!("invalid LSP document selector: {error}");
                        None
                    }
                }
            })
            .collect();
        self.0.insert(R::METHOD, filters);
    }

    pub(crate) fn matches(&self, method: &str, language: &str, uri: Option<&lsp::Uri>) -> bool {
        self.0
            .get(method)
            .is_none_or(|filters| filters.iter().any(|filter| filter.matches(language, uri)))
    }
}

impl Filter {
    fn new(filter: &lsp::TextDocumentFilter) -> anyhow::Result<Self> {
        anyhow::ensure!(
            filter.language.is_some() || filter.scheme.is_some() || filter.pattern.is_some(),
            "empty filter"
        );
        Ok(Self {
            language: filter.language.clone(),
            scheme: filter.scheme.clone(),
            pattern: filter.pattern.as_ref().map(Pattern::new).transpose()?,
        })
    }

    fn matches(&self, language: &str, uri: Option<&lsp::Uri>) -> bool {
        self.language
            .as_deref()
            .is_none_or(|expected| expected == "*" || expected == language)
            && self.scheme.as_deref().is_none_or(|expected| {
                uri.and_then(|uri| uri.scheme()).is_some_and(|scheme| {
                    expected == "*" || expected.eq_ignore_ascii_case(scheme.as_str())
                })
            })
            && self
                .pattern
                .as_ref()
                .is_none_or(|pattern| uri.is_some_and(|uri| pattern.matches(uri)))
    }
}

impl Pattern {
    fn new(pattern: &lsp::GlobPattern) -> anyhow::Result<Self> {
        let (base, glob) = match pattern {
            lsp::GlobPattern::String(pattern) => (None, pattern),
            lsp::GlobPattern::Relative(pattern) => {
                let base = match &pattern.base_uri {
                    lsp::OneOf::Left(folder) => &folder.uri,
                    lsp::OneOf::Right(uri) => uri,
                };
                anyhow::ensure!(
                    base.scheme().is_some(),
                    "relative pattern base must be an absolute URI"
                );
                let path = base
                    .path()
                    .decode()
                    .to_string()
                    .map_err(|_| anyhow::anyhow!("relative pattern base must be UTF-8"))?;
                let prefix = format!("{}/", path.trim_end_matches('/'));
                (Some((base.clone(), prefix)), &pattern.pattern)
            }
        };
        let glob = GlobBuilder::new(glob)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .context("invalid document glob")?;
        let matcher = unicode_glob_regex(glob.regex()).context("invalid document glob")?;
        Ok(Self { base, matcher })
    }

    fn matches(&self, uri: &lsp::Uri) -> bool {
        let decoded_path = uri.path().decode();
        let Ok(path) = decoded_path.to_string() else {
            return false;
        };
        let path = if let Some((base, prefix)) = &self.base {
            let Some(relative_path) = path.strip_prefix(prefix) else {
                return false;
            };
            if !base
                .scheme()
                .zip(uri.scheme())
                .is_some_and(|(base, target)| base.as_str().eq_ignore_ascii_case(target.as_str()))
            {
                return false;
            }
            let authority_matches = if local_file_uri(base) && local_file_uri(uri) {
                true
            } else {
                match (base.authority(), uri.authority()) {
                    (Some(base), Some(target)) => {
                        base.userinfo() == target.userinfo()
                            && base.port() == target.port()
                            && base.host().eq_ignore_ascii_case(target.host())
                    }
                    (None, None) => true,
                    _ => false,
                }
            };
            if !authority_matches {
                return false;
            }
            relative_path
        } else {
            path.as_ref()
        };
        self.matcher.is_match(path)
    }
}

/// Globset escapes UTF-8 literals as individual bytes. Restore those literals and
/// use Unicode regex mode so a single-character wildcard consumes one character.
fn unicode_glob_regex(expression: &str) -> anyhow::Result<Regex> {
    static ESCAPES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\\\|(?:\\x[89a-f][0-9a-f])+").unwrap());

    let expression = expression.strip_prefix("(?-u)").unwrap_or(expression);
    let mut pattern = String::with_capacity(expression.len());
    let mut end = 0;
    for escape in ESCAPES.find_iter(expression) {
        pattern.push_str(&expression[end..escape.start()]);
        let escaped = escape.as_str();
        if escaped == r"\\" {
            pattern.push_str(escaped);
        } else {
            let bytes = (2..escaped.len())
                .step_by(4)
                .map(|offset| u8::from_str_radix(&escaped[offset..offset + 2], 16))
                .collect::<Result<Vec<_>, _>>()?;
            pattern.push_str(std::str::from_utf8(&bytes)?);
        }
        end = escape.end();
    }
    pattern.push_str(&expression[end..]);
    Ok(RegexBuilder::new(&pattern)
        .dot_matches_new_line(true)
        .build()?)
}

fn local_file_uri(uri: &lsp::Uri) -> bool {
    uri.scheme()
        .is_some_and(|scheme| scheme.as_str().eq_ignore_ascii_case("file"))
        && uri.authority().is_none_or(|authority| {
            authority.userinfo().is_none()
                && authority.port().is_none()
                && (authority.host().is_empty()
                    || authority.host().eq_ignore_ascii_case("localhost"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::request::Request as _;
    use serde_json::{json, Value};

    fn compile_selector(selector: Value) -> DocumentSelectors {
        let capabilities = serde_json::from_value(json!({
            "implementationProvider": {"documentSelector": selector, "id": "test"}
        }))
        .unwrap();
        DocumentSelectors::new(&capabilities)
    }

    fn matches(selectors: &DocumentSelectors, language: &str, uri: &str) -> bool {
        let uri = uri.parse().unwrap();
        selectors.matches(
            lsp::request::GotoImplementation::METHOD,
            language,
            Some(&uri),
        )
    }

    #[test]
    fn null_empty_and_notebook_selectors_preserve_binding_semantics() {
        assert!(matches(
            &compile_selector(Value::Null),
            "rust",
            "file:///project/main.rs"
        ));
        assert!(!matches(
            &compile_selector(json!([])),
            "rust",
            "file:///project/main.rs"
        ));
        let notebook = json!({"notebook": {"notebookType": "jupyter"}, "language": "rust", "pattern": "**/*.rs"});
        let filter: lsp::DocumentFilter = serde_json::from_value(notebook.clone()).unwrap();
        assert!(matches!(filter, lsp::DocumentFilter::NotebookCell(_)));
        assert!(!matches(
            &compile_selector(json!([notebook.clone()])),
            "rust",
            "file:///project/main.rs"
        ));
        assert!(matches(
            &compile_selector(json!([notebook, {"language": "rust"}])),
            "rust",
            "file:///project/main.rs"
        ));
    }

    #[test]
    fn constraints_are_conjunctive_and_filters_are_alternatives() {
        let selectors = compile_selector(json!([
            {"language": "rust", "scheme": "file", "pattern": "**/src/*.{rs,rlib}"},
            {"language": "json", "scheme": "untitled"}
        ]));
        for (language, uri, expected) in [
            ("rust", "file:///project/src/main.rs", true),
            ("rust", "file:///project/src/main.rlib", true),
            ("rust", "file:///project/src/deep/main.rs", false),
            ("rust", "file:///project/tests/main.rs", false),
            ("python", "file:///project/src/main.rs", false),
            ("rust", "untitled:/project/src/main.rs", false),
            ("json", "untitled:document-1", true),
        ] {
            assert_eq!(
                matches(&selectors, language, uri),
                expected,
                "{language}: {uri}"
            );
        }
        assert!(matches(
            &compile_selector(json!([{"language":"*", "scheme":"*"}])),
            "rust",
            "git:/project/src/main.rs"
        ));
    }

    #[test]
    fn globs_match_unicode_characters() {
        let wildcard = compile_selector(json!([{"pattern": "**/?.rs"}]));
        assert!(matches(&wildcard, "rust", "file:///project/a.rs"));
        assert!(matches(&wildcard, "rust", "file:///project/%C3%A9.rs"));
        assert!(!matches(&wildcard, "rust", "file:///project/%C3%A9e.rs"));
        assert!(!matches(&wildcard, "rust", "file:///project/.rs"));

        let relative = compile_selector(json!([{
            "pattern": {"baseUri": "file:///project", "pattern": "工具/{[à-ê],ç}.rs"}
        }]));
        assert!(matches(
            &relative,
            "rust",
            "file:///project/%E5%B7%A5%E5%85%B7/%C3%A9.rs"
        ));
        assert!(!matches(
            &relative,
            "rust",
            "file:///project/%E5%B7%A5%E5%85%B7/a.rs"
        ));
    }

    #[test]
    fn relative_bases_are_literal_decoded_directories() {
        for base in [
            json!("file:///project/a%5Bb%5D%20%7Bx%7D%3F*/%E5%B7%A5%E5%85%B7"),
            json!({"name": "workspace", "uri": "file:///project/a%5Bb%5D%20%7Bx%7D%3F*/%E5%B7%A5%E5%85%B7/"}),
        ] {
            let selectors =
                compile_selector(json!([{"pattern": {"baseUri":base,"pattern":"**/*.rs"}}]));
            for (uri, expected) in [
                ("file:///project/a%5Bb%5D%20%7Bx%7D%3F*/%E5%B7%A5%E5%85%B7/main.rs", true),
                ("file:///project/a%5Bb%5D%20%7Bx%7D%3F*/%E5%B7%A5%E5%85%B7/src/main.rs?version=1#L2", true),
                ("file:///project/ab%20x-anything/%E5%B7%A5%E5%85%B7/main.rs", false),
                ("file:///project/a%5Bb%5D%20%7Bx%7D%3F*/%E5%B7%A5%E5%85%B7-extra/main.rs", false),
                ("git:/project/a%5Bb%5D%20%7Bx%7D%3F*/%E5%B7%A5%E5%85%B7/main.rs", false),
            ] {
                assert_eq!(matches(&selectors, "rust", uri), expected, "{uri}");
            }
        }
    }

    #[test]
    fn relative_patterns_preserve_uri_origins_and_windows_paths() {
        for (base, accepted, rejected) in [
            (
                "file:///C:/work/project",
                "file:///C:/work/project/src/main.rs",
                "file:///D:/work/project/src/main.rs",
            ),
            (
                "file://server/share/project",
                "file://SERVER/share/project/main.rs",
                "file://other/share/project/main.rs",
            ),
            (
                "git://host/project",
                "git://HOST/project/main.rs?rev=1#L2",
                "file://host/project/main.rs",
            ),
            (
                "file://localhost/project",
                "file:///project/main.rs",
                "file:///project-other/main.rs",
            ),
            (
                "file:/project",
                "file://localhost/project/main.rs",
                "file://remote/project/main.rs",
            ),
        ] {
            let selectors =
                compile_selector(json!([{"pattern":{"baseUri":base,"pattern":"**/*.rs"}}]));
            assert!(matches(&selectors, "rust", accepted), "{accepted}");
            assert!(!matches(&selectors, "rust", rejected), "{rejected}");
        }
    }

    #[test]
    fn malformed_patterns_do_not_broaden_a_selector() {
        for selector in [
            json!([{"pattern":"["}]),
            json!([{"pattern":{"baseUri":"relative/base", "pattern":"**"}}]),
            json!([{}]),
        ] {
            assert!(!matches(
                &compile_selector(selector),
                "rust",
                "file:///project/main.rs"
            ));
        }
        let selectors = compile_selector(json!([{"pattern":"["},{"language":"rust"}]));
        assert!(matches(&selectors, "rust", "file:///project/main.rs"));
    }

    #[test]
    fn all_supported_static_registration_selectors_are_collected() {
        let mut capabilities = json!({
            "semanticTokensProvider": {"documentSelector":[], "legend":{"tokenTypes":[],"tokenModifiers":[]},"full":true},
            "diagnosticProvider": {"documentSelector":[], "interFileDependencies":false, "workspaceDiagnostics":false}
        });
        for name in [
            "declarationProvider",
            "typeDefinitionProvider",
            "implementationProvider",
            "colorProvider",
            "foldingRangeProvider",
            "selectionRangeProvider",
            "callHierarchyProvider",
            "inlayHintProvider",
            "inlineValueProvider",
            "linkedEditingRangeProvider",
        ] {
            capabilities[name] = json!({"documentSelector":[], "id":name});
        }
        let selectors = DocumentSelectors::new(&serde_json::from_value(capabilities).unwrap());
        assert_eq!(selectors.0.len(), 12);
        for method in selectors.0.keys() {
            assert!(!selectors.matches(
                method,
                "rust",
                Some(&"file:///project/main.rs".parse().unwrap())
            ));
        }
        assert!(selectors.matches("textDocument/hover", "rust", None));
    }
}
