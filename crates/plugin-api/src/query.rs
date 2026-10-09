//! Preflight untrusted query sources before native recursive compilation.

use crate::{ErrorCode, ServiceError};

#[derive(Clone, Copy)]
pub struct QueryLimits {
    pub bytes: usize,
    pub patterns: usize,
}

pub const STRUCTURAL_LIMITS: QueryLimits = QueryLimits {
    bytes: 4096,
    patterns: 64,
};
pub const DECLARATIVE_LIMITS: QueryLimits = QueryLimits {
    bytes: 128 * 1024,
    patterns: 256,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Predicates {
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

/// Comments and quoted text cannot hide syntax work from the counters. Literal
/// predicates compare only bounded strings; regex and capture-text comparisons
/// are excluded. Native grammar/query validation still checks actual syntax.
pub fn validate(
    source: &str,
    limits: QueryLimits,
    predicates: Predicates,
) -> Result<(), ServiceError> {
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
        "#is?" | "#is-not?" => {
            matches!(args, [Token::Atom("local") | Token::String("local")])
        }
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
    fn lexer_bounds_native_compilation_and_literal_predicates() {
        let valid = "; #match? inside comment\n((identifier) @x (#eq? @x \"#(value)\"))";
        assert!(validate(valid, DECLARATIVE_LIMITS, Predicates::Declarative).is_ok());
        assert!(validate(valid, STRUCTURAL_LIMITS, Predicates::Structural).is_err());
        assert!(validate(
            "; # inside comment\n(\"#\") @x",
            STRUCTURAL_LIMITS,
            Predicates::Structural
        )
        .is_ok());
        for source in [
            "((identifier) @x (#match? @x \"a{9999999}\"))".to_owned(),
            "((identifier) @x (#eq? @x @y))".to_owned(),
            "((identifier) @x (#set! external.command \"sh\"))".to_owned(),
            "(".repeat(65),
            "(identifier) @x\n".repeat(257),
            format!("({})", "(identifier)? ".repeat(129)),
            format!("(\"{}\")", "x".repeat(129)),
        ] {
            assert!(
                validate(&source, DECLARATIVE_LIMITS, Predicates::Declarative).is_err(),
                "{source}"
            );
        }
        assert!(validate(
            "((string) @injection.content (#set! injection.language \"json\"))",
            DECLARATIVE_LIMITS,
            Predicates::Declarative
        )
        .is_ok());
    }
}
