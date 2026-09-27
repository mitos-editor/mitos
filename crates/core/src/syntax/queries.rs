//! Compile supplied grammars and query sources. No resource lookup or file I/O.
use super::{
    reconfigure_highlights, Grammar, IndentQuery, Query, RainbowQuery, SpellcheckQuery,
    SyntaxConfig, TagQuery, TextObjectQuery,
};
use anyhow::{Context, Result};
use tree_house::tree_sitter::query::{InvalidPredicateError, UserPredicate};

/// Compile highlighting, injections, and locals for one grammar.
pub fn compile_syntax_config(
    grammar: Grammar,
    name: &str,
    highlights: &str,
    injections: &str,
    locals: &str,
    scopes: &[String],
) -> Result<SyntaxConfig> {
    let config = SyntaxConfig::new(grammar, highlights, injections, locals)
        .with_context(|| format!("Failed to compile highlights for '{name}'"))?;
    reconfigure_highlights(&config, scopes);
    Ok(config)
}

/// Compiles the indents.scm query for a language.
pub fn compile_indent_query(
    grammar: Grammar,
    name: &str,
    text: &str,
) -> Result<Option<IndentQuery>> {
    if text.is_empty() {
        return Ok(None);
    }
    let indent_query = IndentQuery::new(grammar, text)
        .with_context(|| format!("Failed to compile indents.scm query for '{name}'"))?;
    Ok(Some(indent_query))
}

/// Compiles the textobjects.scm query for a language.
pub fn compile_textobject_query(
    grammar: Grammar,
    name: &str,
    text: &str,
) -> Result<Option<TextObjectQuery>> {
    if text.is_empty() {
        return Ok(None);
    }
    let query = Query::new(grammar, text, |_, _| Ok(()))
        .with_context(|| format!("Failed to compile textobjects.scm queries for '{name}'"))?;
    Ok(Some(TextObjectQuery::new(query)))
}

/// Compiles the tags.scm query for a language.
pub fn compile_tag_query(grammar: Grammar, name: &str, text: &str) -> Result<Option<TagQuery>> {
    if text.is_empty() {
        return Ok(None);
    }
    let query = Query::new(grammar, text, |_pattern, predicate| match predicate {
        // TODO: these predicates are allowed in tags.scm queries but not yet used.
        UserPredicate::IsPropertySet { key: "local", .. } => Ok(()),
        UserPredicate::Other(pred) => match pred.name() {
            "strip!" | "select-adjacent!" => Ok(()),
            _ => Err(InvalidPredicateError::unknown(predicate)),
        },
        _ => Err(InvalidPredicateError::unknown(predicate)),
    })
    .with_context(|| format!("Failed to compile tags.scm query for '{name}'"))?;
    Ok(Some(TagQuery { query }))
}

/// Compiles the rainbows.scm query for a language.
pub fn compile_rainbow_query(
    grammar: Grammar,
    name: &str,
    text: &str,
) -> Result<Option<RainbowQuery>> {
    if text.is_empty() {
        return Ok(None);
    }
    let rainbow_query = RainbowQuery::new(grammar, text)
        .with_context(|| format!("Failed to compile rainbows.scm query for '{name}'"))?;
    Ok(Some(rainbow_query))
}

/// Compiles the spellcheck.scm query for a language.
pub fn compile_spellcheck_query(
    grammar: Grammar,
    name: &str,
    text: &str,
) -> Result<Option<SpellcheckQuery>> {
    if text.is_empty() {
        return Ok(None);
    }
    let spellcheck_query = SpellcheckQuery::new(grammar, text)
        .with_context(|| format!("Failed to compile spellcheck.scm query for '{name}'"))?;
    Ok(Some(spellcheck_query))
}
