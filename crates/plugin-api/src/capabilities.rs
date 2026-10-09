use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Declared by a package and independently granted in user configuration.
///
/// A capability names authority, not an implementation function. Adapters must
/// check it even when a builtin command or provider performs the operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    EditorRead,
    EditorEdit,
    EditorSelection,
    EditorNavigate,
    /// Owned setting overrides, restored against the current user baseline.
    EditorSettings,
    Ui,
    WorkspaceRead,
    WorkspaceWrite,
    Storage,
    Process,
    Network,
    Environment,
    Clipboard,
    Provider,
}

pub type CapabilitySet = BTreeSet<Capability>;

/// User-owned restrictions. Workspace configuration cannot broaden these grants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Permissions {
    pub capabilities: CapabilitySet,
    /// Explicit filesystem roots; the current worktree is not an implicit grant.
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub processes: Vec<ProcessGrant>,
    pub environment: BTreeSet<String>,
    /// Exact HTTPS hosts. Redirects and local-network targets require new checks.
    pub network_hosts: BTreeSet<String>,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            capabilities: BTreeSet::from([Capability::Ui]),
            read_roots: Vec::new(),
            write_roots: Vec::new(),
            processes: Vec::new(),
            environment: BTreeSet::new(),
            network_hosts: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProcessGrant {
    /// Exact executable name/path, with no shell parsing or command globbing.
    pub command: String,
    /// Exact argument sequence, or a final `**` to explicitly permit a suffix.
    #[serde(default)]
    pub args: Vec<String>,
}

impl ProcessGrant {
    pub fn allows(&self, command: &str, args: &[String]) -> bool {
        if self.command != command || command.is_empty() {
            return false;
        }
        if self.args.last().is_some_and(|arg| arg == "**") {
            args.starts_with(&self.args[..self.args.len() - 1])
        } else {
            self.args == args
        }
    }
}

impl Permissions {
    pub fn require_process(
        &self,
        declared: &CapabilitySet,
        command: &str,
        args: &[String],
    ) -> Result<(), ServiceError> {
        Capability::Process.require(declared, &self.capabilities)?;
        if self
            .processes
            .iter()
            .any(|grant| grant.allows(command, args))
        {
            Ok(())
        } else {
            Err(ServiceError::new(
                ErrorCode::PermissionDenied,
                "plugin executable or arguments are not granted",
            ))
        }
    }
}

impl Capability {
    /// The effective grant always requires both package and user authorization.
    pub fn require(
        self,
        declared: &CapabilitySet,
        granted: &CapabilitySet,
    ) -> Result<(), ServiceError> {
        if declared.contains(&self) && granted.contains(&self) {
            Ok(())
        } else {
            Err(ServiceError::new(
                ErrorCode::PermissionDenied,
                format!("plugin capability {self:?} is not declared and granted"),
            ))
        }
    }
}

/// Errors are part of the service contract, independently of the WASM engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    InvalidRequest,
    StaleState,
    PermissionDenied,
    ResourceExhausted,
    Cancelled,
    DeadlineExceeded,
    UnsupportedInterface,
    GuestTrap,
    HostFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ServiceError {
    pub code: ErrorCode,
    pub message: String,
}

impl ServiceError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        let message = message.into();
        let mut bounded = String::new();
        for character in message.chars() {
            let character = if character.is_control() {
                ' '
            } else {
                character
            };
            if bounded.len() + character.len_utf8() > 4096 {
                break;
            }
            bounded.push(character);
        }
        Self {
            code,
            message: bounded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neither_declarations_nor_user_grants_can_authorize_alone() {
        let read = BTreeSet::from([Capability::EditorRead]);
        let none = BTreeSet::new();
        for (declared, granted) in [(&read, &none), (&none, &read), (&none, &none)] {
            assert_eq!(
                Capability::EditorRead
                    .require(declared, granted)
                    .unwrap_err()
                    .code,
                ErrorCode::PermissionDenied
            );
        }
        assert!(Capability::EditorRead.require(&read, &read).is_ok());
        assert!(Capability::Process.require(&read, &read).is_err());
    }

    #[test]
    fn external_authority_is_denied_and_process_rules_are_exact() {
        let permissions = Permissions::default();
        assert_eq!(permissions.capabilities, BTreeSet::from([Capability::Ui]));
        assert!(permissions.read_roots.is_empty());
        let declared = BTreeSet::from([Capability::Process]);
        assert!(permissions
            .require_process(&declared, "rustfmt", &[])
            .is_err());
        let mut granted = permissions;
        granted.capabilities.insert(Capability::Process);
        granted.processes.push(ProcessGrant {
            command: "rustfmt".into(),
            args: vec!["--emit".into(), "stdout".into()],
        });
        assert!(granted
            .require_process(&declared, "rustfmt", &["--emit".into(), "stdout".into()])
            .is_ok());
        assert!(granted.require_process(&declared, "sh", &[]).is_err());
        assert!(granted
            .require_process(
                &declared,
                "rustfmt",
                &["--emit".into(), "stdout".into(), "file.rs".into()]
            )
            .is_err());
        assert!(granted
            .require_process(
                &BTreeSet::new(),
                "rustfmt",
                &["--emit".into(), "stdout".into()]
            )
            .is_err());
    }
}
