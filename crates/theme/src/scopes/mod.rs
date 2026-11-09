//! Public highlight vocabulary and selector matching shared by themes and tools.
//!
//! Kinds form a hierarchy; qualifiers describe independent occurrence roles and
//! symbol traits. Query helper captures are deliberately outside this vocabulary.

/// A documented highlight kind. More specific kinds inherit broader styling.
pub const KINDS: &[(&str, &str)] = &[
    (
        "attribute",
        "Attributes and decorators; markup tag attributes.",
    ),
    ("comment", "Comments."),
    ("comment.block", "Block comments."),
    ("comment.error", "Error annotations in comments."),
    ("comment.hint", "Hints in comments."),
    ("comment.info", "Informational annotations in comments."),
    ("comment.line", "Line comments."),
    (
        "comment.unused",
        "Intentionally unused bindings and patterns.",
    ),
    ("comment.warning", "Warning annotations in comments."),
    ("constant", "Named constants."),
    ("constant.character", "Character literals."),
    ("constant.character.escape", "Escape sequences."),
    ("constant.macro", "Preprocessor constants."),
    ("constant.numeric", "Numeric literals."),
    ("constant.numeric.float", "Floating-point literals."),
    ("constant.numeric.integer", "Integer literals."),
    (
        "constructor",
        "Constructors and record literals in value position.",
    ),
    ("diff", "Version-control changes."),
    ("diff.delta", "Modifications."),
    ("diff.delta.conflict", "Conflicts."),
    ("diff.delta.moved", "Moves and renames."),
    ("diff.minus", "Deletions."),
    ("diff.plus", "Additions."),
    ("embedded", "Embedded expressions."),
    ("error", "Error severity."),
    (
        "function",
        "Functions, including references with no known occurrence role.",
    ),
    ("function.macro", "Macros."),
    ("function.method", "Member functions and methods."),
    (
        "function.special",
        "Special compiler or preprocessor functions.",
    ),
    ("hint", "Hint severity."),
    ("info", "Information severity."),
    ("keyword", "Language keywords."),
    ("keyword.control", "Control-flow keywords."),
    ("keyword.control.conditional", "Conditional keywords."),
    ("keyword.control.exception", "Exception-handling keywords."),
    ("keyword.control.import", "Import and export keywords."),
    ("keyword.control.repeat", "Loop keywords."),
    ("keyword.control.return", "Return keywords."),
    (
        "keyword.coroutine",
        "Coroutine and asynchronous control keywords.",
    ),
    ("keyword.directive", "Preprocessor directives."),
    ("keyword.function", "Function declaration keywords."),
    ("keyword.operator", "Word operators."),
    ("keyword.storage", "Storage and declaration keywords."),
    ("keyword.storage.modifier", "Storage modifiers."),
    ("keyword.storage.modifier.mut", "Mutability modifiers."),
    ("keyword.storage.modifier.ref", "Reference modifiers."),
    (
        "keyword.storage.type",
        "Type and binding declaration keywords.",
    ),
    ("label", "Labels, including CSS selectors."),
    ("markup", "Markup text."),
    ("markup.bold", "Bold text."),
    ("markup.heading", "Headings."),
    ("markup.heading.1", "Level 1 heading text."),
    ("markup.heading.2", "Level 2 heading text."),
    ("markup.heading.3", "Level 3 heading text."),
    ("markup.heading.4", "Level 4 heading text."),
    ("markup.heading.5", "Level 5 heading text."),
    ("markup.heading.6", "Level 6 heading text."),
    ("markup.heading.marker", "Heading markers."),
    ("markup.italic", "Italic text."),
    ("markup.link", "Links."),
    ("markup.link.label", "Link reference labels."),
    ("markup.link.text", "Link descriptions."),
    ("markup.link.url", "Link destinations."),
    ("markup.list", "Lists."),
    ("markup.list.checked", "Checked items."),
    ("markup.list.numbered", "Ordered lists."),
    ("markup.list.unchecked", "Unchecked items."),
    ("markup.list.unnumbered", "Unordered lists."),
    ("markup.normal", "Ordinary rendered documentation."),
    ("markup.quote", "Quotations."),
    ("markup.raw", "Raw text and code."),
    ("markup.raw.block", "Code blocks."),
    ("markup.raw.inline", "Inline code."),
    ("markup.strikethrough", "Strikethrough text."),
    ("markup.underline", "Underlined text."),
    ("namespace", "Namespaces, packages and modules."),
    ("operator", "Symbolic operators."),
    ("operator.control", "Control operators."),
    ("operator.special", "Language-specific operators."),
    ("punctuation", "Punctuation."),
    ("punctuation.bracket", "Brackets and parentheses."),
    ("punctuation.delimiter", "Separators and delimiters."),
    (
        "punctuation.special",
        "Interpolation and other special punctuation.",
    ),
    ("special", "Legacy language-specific syntax."),
    ("string", "String literals."),
    ("string.code", "Code strings."),
    ("string.regexp", "Regular expressions."),
    ("string.special", "Special string literals."),
    ("string.special.format", "Formatting placeholders."),
    ("string.special.path", "Paths."),
    ("string.special.subscript", "Subscript text."),
    ("string.special.superscript", "Superscript text."),
    ("string.special.symbol", "Atoms and symbols."),
    ("string.special.url", "URLs."),
    ("tag", "Markup tags."),
    ("tag.delimiter", "Markup tag delimiters."),
    ("tag.special", "Special markup tags."),
    ("type", "Types."),
    ("type.alias", "Type aliases."),
    ("type.class", "Classes."),
    ("type.enum", "Enumeration types."),
    ("type.enum.variant", "Enumeration variants."),
    ("type.interface", "Interfaces."),
    ("type.parameter", "Generic type parameters."),
    ("type.struct", "Structs and records."),
    ("type.trait", "Traits."),
    ("type.union", "Unions."),
    ("variable", "Variables."),
    ("variable.other", "Other variable-like identifiers."),
    ("variable.other.member", "Fields and member properties."),
    ("variable.parameter", "Function and method parameters."),
    ("variable.special", "Language-specific variables."),
    ("warning", "Warning severity."),
];

/// Qualifiers match independently of their order after a kind.
/// `call` is never inferred from the absence of `definition` or `declaration`.
pub const QUALIFIERS: &[(&str, &str)] = &[
    (
        "declaration",
        "A symbol declaration or signature without a known definition body.",
    ),
    ("definition", "A symbol definition."),
    (
        "reference",
        "A symbol reference that is not classified more specifically.",
    ),
    (
        "call",
        "An invocation, rather than a reference to a callable value.",
    ),
    ("modification", "A write to a symbol."),
    ("builtin", "A language or standard-library symbol."),
    ("boolean", "A built-in boolean constant."),
    ("mutable", "A mutable binding."),
    ("readonly", "A read-only binding or member."),
    ("public", "Public or exported visibility."),
    ("private", "Private visibility."),
    ("static", "A static member."),
    ("async", "An asynchronous callable."),
    ("abstract", "An abstract symbol."),
    ("deprecated", "A deprecated symbol."),
    (
        "documentation",
        "Documentation text or a symbol occurrence in documentation.",
    ),
    ("special", "A language-specific specialization."),
];

/// A parsed kind and independent qualifiers. Unknown names remain valid custom
/// scopes at runtime, but repository tooling rejects them in bundled resources.
#[derive(Debug)]
pub struct Classification<'a> {
    pub kind: &'a str,
    pub qualifiers: Vec<&'a str>,
}

impl<'a> Classification<'a> {
    pub fn parse(scope: &'a str) -> Option<Self> {
        if scope.ends_with('.') || scope.contains("..") {
            return None;
        }
        let mut prefix = scope;
        let kind = loop {
            if KINDS
                .binary_search_by_key(&prefix, |(name, _)| *name)
                .is_ok()
            {
                break prefix;
            }
            prefix = prefix.rsplit_once('.')?.0;
        };
        let qualifiers: Vec<_> = scope[kind.len()..]
            .strip_prefix('.')
            .unwrap_or("")
            .split('.')
            .filter(|part| !part.is_empty())
            .collect();
        if qualifiers.iter().enumerate().any(|(index, part)| {
            !QUALIFIERS.iter().any(|(name, _)| name == part) || qualifiers[..index].contains(part)
        }) {
            return None;
        }
        Some(Self { kind, qualifiers })
    }
}

/// Specificity is ordered by kind depth, then qualifier count. Equal-specificity
/// rules are applied in lexical order, making conflicts deterministic.
pub fn specificity(selector: &str, capture: &str) -> Option<(usize, usize)> {
    specificity_for(selector, &Classification::parse(capture)?)
}

pub fn specificity_for(selector: &str, capture: &Classification<'_>) -> Option<(usize, usize)> {
    let (kind, qualifiers) = if let Some(rest) = selector.strip_prefix("*.") {
        ("", rest.split('.').collect::<Vec<_>>())
    } else {
        let selector = Classification::parse(selector)?;
        (selector.kind, selector.qualifiers)
    };
    if !kind.is_empty()
        && capture.kind != kind
        && !capture
            .kind
            .strip_prefix(kind)
            .is_some_and(|rest| rest.starts_with('.'))
    {
        return None;
    }
    qualifiers
        .iter()
        .all(|q| capture.qualifiers.contains(q))
        .then_some((
            if kind.is_empty() {
                0
            } else {
                kind.split('.').count()
            },
            qualifiers.len(),
        ))
}

pub fn is_syntax_selector(name: &str) -> bool {
    if let Some(rest) = name.strip_prefix("*.") {
        let parts: Vec<_> = rest.split('.').collect();
        !parts.is_empty()
            && parts.iter().enumerate().all(|(index, part)| {
                QUALIFIERS.iter().any(|(q, _)| q == part) && !parts[..index].contains(part)
            })
    } else {
        Classification::parse(name).is_some()
    }
}

mod interface;
pub use interface::INTERFACE;

mod captures;
pub use captures::CAPTURES;

/// Unknown custom scopes are accepted by the runtime loader. This strict check
/// is for bundled resources, where spelling mistakes must fail CI.
pub fn is_known(name: &str) -> bool {
    is_syntax_selector(name) || INTERFACE.iter().any(|(key, _)| *key == name)
}

/// Refine a syntactic occurrence with server-resolved symbol information. A
/// server's unspecified role preserves syntactic call/reference information;
/// explicit semantic roles and contradictory traits take precedence.
pub fn refine(syntax: Option<&str>, semantic: &str) -> String {
    let Some(mut semantic_class) = Classification::parse(semantic) else {
        return semantic.into();
    };
    let syntax_class = syntax.and_then(Classification::parse);
    // LSP declaration also covers definitions for many servers. A generic
    // declaration flag must not discard a body established by the syntax tree.
    if syntax_class
        .as_ref()
        .is_some_and(|syntax| syntax.qualifiers.contains(&"definition"))
        && semantic_class.qualifiers.contains(&"declaration")
        && !semantic_class.qualifiers.contains(&"definition")
    {
        semantic_class.qualifiers.retain(|q| *q != "declaration");
        semantic_class.qualifiers.push("definition");
    }
    if semantic_class.qualifiers.contains(&"definition") {
        semantic_class.qualifiers.retain(|q| *q != "declaration");
    }
    let roles = [
        "declaration",
        "definition",
        "call",
        "reference",
        "modification",
    ];
    let semantic_role = semantic_class.qualifiers.iter().any(|q| roles.contains(q));
    let mut qualifiers = std::collections::BTreeSet::new();
    if let Some(syntax) = &syntax_class {
        for qualifier in &syntax.qualifiers {
            if semantic_role && roles.contains(qualifier) {
                continue;
            }
            if *qualifier == "builtin" && !semantic_class.qualifiers.contains(&"builtin") {
                continue;
            }
            if (*qualifier == "mutable" && semantic_class.qualifiers.contains(&"readonly"))
                || (*qualifier == "readonly" && semantic_class.qualifiers.contains(&"mutable"))
            {
                continue;
            }
            qualifiers.insert(*qualifier);
        }
    }
    qualifiers.extend(semantic_class.qualifiers);
    let mut result = semantic_class.kind.to_owned();
    for qualifier in qualifiers {
        result.push('.');
        result.push_str(qualifier);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_refinement_preserves_occurrences_and_corrects_guessed_builtins() {
        assert_eq!(
            refine(Some("function.method.call"), "function"),
            "function.call"
        );
        assert_eq!(
            refine(
                Some("function.public.definition"),
                "function.method.declaration"
            ),
            "function.method.definition.public"
        );
        assert_eq!(
            refine(Some("function.builtin.call"), "variable"),
            "variable.call"
        );
        assert_eq!(
            refine(Some("variable.mutable"), "variable.readonly"),
            "variable.readonly"
        );
    }

    #[test]
    fn qualifiers_are_independent_and_custom_scopes_remain_distinct() {
        assert!(KINDS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(
            specificity(
                "function.method.public",
                "function.method.definition.public"
            ),
            Some((2, 1))
        );
        assert_eq!(
            specificity("*.definition", "function.method.public.definition"),
            Some((0, 1))
        );
        assert_eq!(
            specificity("function.call", "function.method.reference"),
            None
        );
        assert_eq!(specificity("function.method", "functionish.method"), None);
        assert!(Classification::parse("function.method.defintion").is_none());
        assert!(Classification::parse("function.definition.definition").is_none());
    }
}
