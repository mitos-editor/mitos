//! Source-complexity admission before the non-interruptible native compiler.

use plugin_api::{ErrorCode, ServiceError};
use wasmparser::{CompositeInnerType, Operator, Parser, Payload, TypeRef};

const MAX_FUNCTIONS: u32 = 10_000;
const MAX_OPERATORS: usize = 1_000_000;
const MAX_LOCALS: usize = 100_000;
const MAX_FUNCTION_LOCALS: usize = 4096;
const MAX_SECTIONS: usize = 10_000;
const MAX_MODULE_DEPTH: usize = 16;
const MAX_CONTROL_DEPTH: usize = 256;
const MAX_TYPE_FIELDS: usize = 100_000;
const MAX_SIGNATURE_FIELDS: usize = 256;

fn limit() -> ServiceError {
    ServiceError::new(
        ErrorCode::ResourceExhausted,
        "plugin component exceeds compiler complexity limits",
    )
}
fn malformed(error: wasmparser::BinaryReaderError) -> ServiceError {
    ServiceError::new(
        ErrorCode::InvalidRequest,
        format!("invalid plugin component: {error}"),
    )
}

pub(crate) fn check(bytes: &[u8]) -> Result<(), ServiceError> {
    let mut functions = 0u32;
    let mut operators = 0usize;
    let mut locals = 0usize;
    let mut sections = 0usize;
    let mut depth = 0usize;
    let mut types = 0usize;
    let mut type_fields = 0usize;
    for payload in Parser::new(0).parse_all(bytes) {
        sections += 1;
        if sections > MAX_SECTIONS {
            return Err(limit());
        }
        match payload.map_err(malformed)? {
            Payload::Version { .. } => {
                depth += 1;
                if depth > MAX_MODULE_DEPTH {
                    return Err(limit());
                }
            }
            Payload::End(_) => {
                depth = depth.saturating_sub(1);
            }
            Payload::FunctionSection(section) => {
                functions = functions.checked_add(section.count()).ok_or_else(limit)?;
                if functions > MAX_FUNCTIONS {
                    return Err(limit());
                }
            }
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    if matches!(
                        import.map_err(malformed)?.ty,
                        TypeRef::Func(_) | TypeRef::FuncExact(_)
                    ) {
                        functions = functions.checked_add(1).ok_or_else(limit)?;
                        if functions > MAX_FUNCTIONS {
                            return Err(limit());
                        }
                    }
                }
            }
            Payload::TypeSection(section) => {
                for group in section {
                    let group = group.map_err(malformed)?;
                    types = types.checked_add(group.types().len()).ok_or_else(limit)?;
                    if types > MAX_FUNCTIONS as usize {
                        return Err(limit());
                    }
                    for ty in group.types() {
                        let fields = match &ty.composite_type.inner {
                            CompositeInnerType::Func(function) => {
                                let fields = function
                                    .params()
                                    .len()
                                    .saturating_add(function.results().len());
                                if fields > MAX_SIGNATURE_FIELDS {
                                    return Err(limit());
                                }
                                fields
                            }
                            CompositeInnerType::Struct(structure) => structure.fields.len(),
                            CompositeInnerType::Array(_) | CompositeInnerType::Cont(_) => 1,
                        };
                        type_fields = type_fields.checked_add(fields).ok_or_else(limit)?;
                        if type_fields > MAX_TYPE_FIELDS {
                            return Err(limit());
                        }
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let declarations = body.get_locals_reader().map_err(malformed)?;
                if declarations.get_count() > MAX_FUNCTION_LOCALS as u32 {
                    return Err(limit());
                }
                let mut function_locals = 0usize;
                for declaration in declarations {
                    let (count, _) = declaration.map_err(malformed)?;
                    function_locals = function_locals
                        .checked_add(count as usize)
                        .ok_or_else(limit)?;
                    if function_locals > MAX_FUNCTION_LOCALS {
                        return Err(limit());
                    }
                }
                locals = locals.checked_add(function_locals).ok_or_else(limit)?;
                if locals > MAX_LOCALS {
                    return Err(limit());
                }
                let mut reader = body.get_operators_reader().map_err(malformed)?;
                let mut control = 0usize;
                while !reader.eof() {
                    operators += 1;
                    if operators > MAX_OPERATORS {
                        return Err(limit());
                    }
                    match reader.read().map_err(malformed)? {
                        Operator::Block { .. }
                        | Operator::Loop { .. }
                        | Operator::If { .. }
                        | Operator::Try { .. }
                        | Operator::TryTable { .. } => {
                            control += 1;
                            if control > MAX_CONTROL_DEPTH {
                                return Err(limit());
                            }
                        }
                        Operator::End => control = control.saturating_sub(1),
                        Operator::BrTable { targets } if targets.len() > 4096 => {
                            return Err(limit())
                        }
                        _ => (),
                    }
                }
            }
            _ => (),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_excessive_locals_and_control_depth_before_compilation() {
        let locals = wat::parse_str(format!(
            "(module (func (local {})))",
            "i32 ".repeat(MAX_FUNCTION_LOCALS + 1)
        ))
        .unwrap();
        assert_eq!(
            check(&locals).unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        let nested = wat::parse_str(format!(
            "(module (func {} {}))",
            "block ".repeat(MAX_CONTROL_DEPTH + 1),
            "end ".repeat(MAX_CONTROL_DEPTH + 1)
        ))
        .unwrap();
        assert_eq!(
            check(&nested).unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        let signature = wat::parse_str(format!(
            "(module (type (func (param {}))))",
            "i32 ".repeat(MAX_SIGNATURE_FIELDS + 1)
        ))
        .unwrap();
        assert_eq!(
            check(&signature).unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        check(&wat::parse_str("(module (func (local i32) nop))").unwrap()).unwrap();
    }
}
