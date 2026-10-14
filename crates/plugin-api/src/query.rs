//! Preflight untrusted query sources before native recursive compilation.

use crate::{ErrorCode, ServiceError};

#[derive(Clone, Copy)]
struct QueryLimits {
    bytes: usize,
    patterns: usize,
}

/// Native grammar validation must also reject more structural query patterns.
pub const MAX_STRUCTURAL_PATTERNS: usize = 64;

const STRUCTURAL_LIMITS: QueryLimits = QueryLimits {
    bytes: crate::editor::MAX_EDITOR_REQUEST_BYTES,
    patterns: MAX_STRUCTURAL_PATTERNS,
};
const DECLARATIVE_LIMITS: QueryLimits = QueryLimits {
    bytes: crate::assets::MAX_SOURCE_BYTES,
    patterns: 256,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Predicates {
    Structural,
    Declarative,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Open(u8),
    Close(u8),
    Atom(&'a str),
    String(&'a str),
    Capture(&'a str),
    Predicate(&'a str),
    Quantifier,
}

fn exhausted(message: &str) -> ServiceError {
    ServiceError::new(ErrorCode::ResourceExhausted, message)
}
fn invalid(message: &str) -> ServiceError {
    ServiceError::new(ErrorCode::InvalidRequest, message)
}
fn unsupported() -> ServiceError {
    ServiceError::new(
        ErrorCode::UnsupportedInterface,
        "query predicate is outside the supported literal/property/position subset",
    )
}

/// Preflight an editor-service query with fixed source, nesting and work bounds.
/// Predicates are excluded; comments and quoted text do not count as predicates.
/// Native grammar validation must still check actual syntax and capture counts.
pub fn validate_structural(source: &str) -> Result<(), ServiceError> {
    validate(source, STRUCTURAL_LIMITS, Predicates::Structural)
}

/// Preflight a contributed query with fixed source, nesting and work bounds.
/// Literal predicates compare only bounded strings; regex, capture-text
/// comparisons and native text lookup roles are excluded. Native grammar
/// validation must still check actual syntax and capture counts.
pub fn validate_declarative(source: &str) -> Result<(), ServiceError> {
    validate(source, DECLARATIVE_LIMITS, Predicates::Declarative)
}

fn validate(source: &str, limits: QueryLimits, predicates: Predicates) -> Result<(), ServiceError> {
    if source.len() > limits.bytes {
        return Err(exhausted("query source exceeds byte bound"));
    }
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut stack = Vec::new();
    let mut tokens = Vec::new();
    let (mut patterns, mut captures, mut quantifiers) = (0, 0, 0);
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let token = match byte {
            b';' => {
                let start = index + 1;
                index = bytes[index..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(bytes.len(), |end| index + end);
                if predicates == Predicates::Declarative
                    && source[start..index].trim_start().starts_with("inherits")
                {
                    return Err(invalid("contributed query inheritance is unsupported"));
                }
                continue;
            }
            b'(' | b'[' => {
                if stack.is_empty() {
                    patterns += 1;
                    if patterns > limits.patterns {
                        return Err(exhausted("query pattern count exceeds bound"));
                    }
                }
                stack.push(byte);
                if stack.len() > 64 {
                    return Err(exhausted("query nesting exceeds 64"));
                }
                index += 1;
                Token::Open(byte)
            }
            b')' | b']' => {
                if stack.pop() != Some(if byte == b')' { b'(' } else { b'[' }) {
                    return Err(invalid("query delimiters are unbalanced"));
                }
                index += 1;
                Token::Close(byte)
            }
            b'"' => {
                index += 1;
                let start = index;
                let mut escaped = false;
                while index < bytes.len() {
                    let byte = bytes[index];
                    if !escaped && byte == b'"' {
                        break;
                    }
                    escaped = !escaped && byte == b'\\';
                    index += 1;
                    if index - start > 128 {
                        return Err(exhausted("query literal exceeds 128 bytes"));
                    }
                }
                if index == bytes.len() {
                    return Err(invalid("query string is unclosed"));
                }
                let value = &source[start..index];
                index += 1;
                Token::String(value)
            }
            b'?' | b'*' | b'+' => {
                quantifiers += 1;
                if quantifiers > 128 {
                    return Err(exhausted("query quantifier count exceeds 128"));
                }
                index += 1;
                Token::Quantifier
            }
            _ => {
                let start = index;
                let prefix = byte;
                index += 1;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(
                        bytes[index],
                        b'(' | b')' | b'[' | b']' | b'"' | b';' | b'@' | b'#'
                    )
                    && (prefix == b'#' || !matches!(bytes[index], b'?' | b'*' | b'+'))
                {
                    index += 1;
                }
                if index - start > 128 {
                    return Err(exhausted("query name exceeds 128 bytes"));
                }
                let value = &source[start..index];
                match prefix {
                    b'@' => {
                        if predicates == Predicates::Declarative
                            && (value == "@local.reference"
                                || value.starts_with("@local.definition.")
                                || matches!(
                                    value,
                                    "@injection.language"
                                        | "@injection.filename"
                                        | "@injection.shebang"
                                ))
                        {
                            return Err(ServiceError::new(
                                ErrorCode::UnsupportedInterface,
                                format!(
                                    "query capture '{value}' requires unbounded native text lookup; use ordinary highlight captures or a literal injection.language property"
                                ),
                            ));
                        }
                        captures += 1;
                        if captures > 1024 {
                            return Err(exhausted("query capture uses exceed 1024"));
                        }
                        Token::Capture(value)
                    }
                    b'#' => Token::Predicate(value),
                    _ => Token::Atom(value),
                }
            }
        };
        if stack.is_empty() && matches!(token, Token::String(_) | Token::Atom(_)) {
            patterns += 1;
            if patterns > limits.patterns {
                return Err(exhausted("query pattern count exceeds bound"));
            }
        }
        if tokens.len() == 4096 {
            return Err(exhausted("query token count exceeds 4096"));
        }
        tokens.push(token);
    }
    if !stack.is_empty() {
        return Err(invalid("query delimiters are unclosed"));
    }
    for (index, token) in tokens.iter().enumerate() {
        let Token::Predicate(name) = token else {
            continue;
        };
        if predicates == Predicates::Structural {
            return Err(unsupported());
        }
        if index == 0 || tokens[index - 1] != Token::Open(b'(') {
            return Err(invalid("query predicate requires its own group"));
        }
        let args = &tokens[index + 1..];
        let end = args
            .iter()
            .position(|token| matches!(token, Token::Close(b')')))
            .ok_or_else(|| invalid("query predicate group is unclosed"))?;
        validate_predicate(name, &args[..end])?;
    }
    Ok(())
}

fn validate_predicate(name: &str, args: &[Token<'_>]) -> Result<(), ServiceError> {
    if matches!(name, "#is?" | "#is-not?")
        && matches!(args, [Token::Atom("local") | Token::String("local")])
    {
        return Err(ServiceError::new(
            ErrorCode::UnsupportedInterface,
            "local lookup predicates require unbounded native capture text; use literal or node-position predicates",
        ));
    }
    let literal = |arg| matches!(arg, Token::String(_));
    let capture = |arg| matches!(arg, Token::Capture(_));
    let accepted = match name {
        "#eq?" | "#not-eq?" | "#any-eq?" | "#any-not-eq?" | "#not-kind-eq?" => {
            args.len() == 2 && capture(args[0]) && literal(args[1])
        }
        "#any-of?" | "#not-any-of?" => {
            (2..=33).contains(&args.len())
                && capture(args[0])
                && args[1..].iter().copied().all(literal)
        }
        "#same-line?" | "#not-same-line?" => args.len() == 2 && args.iter().copied().all(capture),
        "#one-line?" | "#not-one-line?" => args.len() == 1 && capture(args[0]),
        "#set!" => match args {
            [Token::Atom(key) | Token::String(key)] => matches!(
                *key,
                "injection.combined"
                    | "injection.include-children"
                    | "injection.include-unnamed-children"
                    | "rainbow.include-children"
            ),
            [Token::Atom(key) | Token::String(key), Token::String(value)] => match *key {
                "injection.language" => crate::assets::valid_name(value),
                "local.scope-inherits" => matches!(*value, "true" | "false"),
                "scope" => *value == "header",
                _ => false,
            },
            _ => false,
        },
        _ => false,
    };
    if accepted {
        Ok(())
    } else {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_points_keep_fixed_source_and_pattern_limits() {
        let source = "(identifier) @name\n".repeat(MAX_STRUCTURAL_PATTERNS + 1);
        assert_eq!(
            validate_structural(&source).unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        validate_declarative(&source).unwrap();
        for (validate, bound) in [
            (
                validate_structural as fn(&str) -> Result<(), ServiceError>,
                STRUCTURAL_LIMITS.bytes,
            ),
            (validate_declarative, DECLARATIVE_LIMITS.bytes),
        ] {
            let source = " ".repeat(bound);
            validate(&source).unwrap();
            assert_eq!(
                validate(&(source + " ")).unwrap_err().code,
                ErrorCode::ResourceExhausted
            );
        }
    }

    #[test]
    fn lexer_bounds_native_compilation_and_literal_predicates() {
        let valid = "; #match? inside comment\n((identifier) @x (#eq? @x \"#(value)\"))";
        assert!(validate_declarative(valid).is_ok());
        assert!(validate_structural(valid).is_err());
        assert!(validate_structural("; # inside comment\n(\"#\") @x").is_ok());
        for source in [
            "((identifier) @x (#match? @x \"a{9999999}\"))".to_owned(),
            "((identifier) @x (#eq? @x @y))".to_owned(),
            "((identifier) @x (#set! external.command \"sh\"))".to_owned(),
            "(".repeat(65),
            "(identifier) @x\n".repeat(257),
            format!("({})", "(identifier)? ".repeat(129)),
            format!("(\"{}\")", "x".repeat(129)),
        ] {
            assert!(validate_declarative(&source).is_err(), "{source}");
        }
        assert!(validate_declarative(
            "((string) @injection.content (#set! injection.language \"json\"))"
        )
        .is_ok());
    }

    #[test]
    fn declarative_queries_reject_native_text_lookup_roles() {
        for capture in [
            "local.reference",
            "local.definition.variable",
            "injection.language",
            "injection.filename",
            "injection.shebang",
        ] {
            // Even a one-pattern query could capture the entire document. A
            // structural service only returns bounded node metadata, so these
            // same capture labels remain valid there.
            let source = format!("(document) @{capture}");
            let error = validate_declarative(&source).unwrap_err();
            assert_eq!(error.code, ErrorCode::UnsupportedInterface);
            assert!(error.message.contains(capture));
            assert!(validate_structural(&source).is_ok());
        }
        for predicate in ["#is? local", "#is-not? \"local\""] {
            let source = format!("((document) @variable ({predicate}))");
            let error = validate_declarative(&source).unwrap_err();
            assert_eq!(error.code, ErrorCode::UnsupportedInterface);
            assert!(error.message.contains("local lookup"));
        }
        assert!(validate_declarative("((document) @variable (#not-eq? @variable \"tiny\"))\n((string) @injection.content (#set! injection.language \"json\"))")
        .is_ok());
    }
}
