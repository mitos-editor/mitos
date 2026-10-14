//! Static argument metadata for native command parsing and completion.

use serde::{Deserialize, Serialize};

use crate::{ErrorCode, ServiceError};

pub const MAX_COMMAND_ARGS: usize = 32;
pub const MAX_ARGUMENT_CANDIDATES: usize = 32;
pub const MAX_ARGUMENT_CANDIDATE_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandArguments {
    pub min: usize,
    pub max: usize,
    /// Native literal suggestions indexed by positional argument. These cannot
    /// request filesystem, shell, guest execution or expansion completion.
    pub completions: Vec<Vec<String>>,
}

impl Default for CommandArguments {
    fn default() -> Self {
        Self {
            min: 0,
            max: MAX_COMMAND_ARGS,
            completions: Vec::new(),
        }
    }
}

impl CommandArguments {
    pub fn validate(&self) -> Result<(), ServiceError> {
        if self.min > self.max
            || self.max > MAX_COMMAND_ARGS
            || self.completions.len() > self.max
            || self.completions.iter().any(|position| {
                position.len() > MAX_ARGUMENT_CANDIDATES
                    || position.iter().any(|candidate| {
                        candidate.is_empty()
                            || candidate.len() > MAX_ARGUMENT_CANDIDATE_BYTES
                            || candidate.chars().any(char::is_control)
                    })
            })
        {
            return Err(ServiceError::new(
                ErrorCode::InvalidRequest,
                "invalid static command argument metadata",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_arguments_have_bounded_counts_and_literal_candidates() {
        assert!(CommandArguments {
            min: 2,
            max: 1,
            completions: vec![]
        }
        .validate()
        .is_err());
        assert!(CommandArguments {
            max: 1,
            completions: vec![vec!["\x1b[31m".into()]],
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(CommandArguments {
            min: 1,
            max: 1,
            completions: vec![vec!["two words".into(), "μ".into()]]
        }
        .validate()
        .is_ok());
    }
}
