//! Authority bound to a loaded package and revoked with its generation.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use plugin_api::{
    Capability, CapabilitySet, ErrorCode, Permissions, ReadRoot, ServiceError, MAX_READ_ROOTS,
    MAX_READ_ROOT_METADATA_BYTES,
};

use crate::filesystem::ScopedDirectory;

pub struct AccessPolicy {
    pub declared: CapabilitySet,
    pub permissions: Permissions,
    read_roots: Vec<(PathBuf, PathBuf, ScopedDirectory)>,
    write_roots: Vec<(PathBuf, PathBuf, ScopedDirectory)>,
    revoked: AtomicBool,
}

impl AccessPolicy {
    pub fn new(
        declared: CapabilitySet,
        permissions: Permissions,
        base: &Path,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            permissions.read_roots.len() <= MAX_READ_ROOTS && permissions.write_roots.len() <= 16,
            "plugin grants exceed the directory limit"
        );
        anyhow::ensure!(
            permissions.processes.len() <= 32
                && permissions.environment.len() <= 32
                && permissions.network_hosts.len() <= 64,
            "plugin grants exceed the rule limit"
        );
        let roots =
            |paths: &[PathBuf]| -> anyhow::Result<Vec<(PathBuf, PathBuf, ScopedDirectory)>> {
                paths
                    .iter()
                    .map(|path| {
                        let configured = std::path::absolute(base.join(path))?;
                        let path = configured.canonicalize()?;
                        let directory = ScopedDirectory::open(&path)?;
                        Ok((path, configured, directory))
                    })
                    .collect()
            };
        let read_roots = roots(&permissions.read_roots)?;
        let write_roots = roots(&permissions.write_roots)?;
        Ok(Self {
            declared,
            permissions,
            read_roots,
            write_roots,
            revoked: AtomicBool::new(false),
        })
    }

    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    pub fn require(&self, capability: Capability) -> Result<(), ServiceError> {
        self.check_live()?;
        capability.require(&self.declared, &self.permissions.capabilities)
    }

    pub fn check_live(&self) -> Result<(), ServiceError> {
        if self.revoked.load(Ordering::Acquire) {
            Err(ServiceError::new(
                ErrorCode::Cancelled,
                "plugin grant has been revoked",
            ))
        } else {
            Ok(())
        }
    }

    /// Reads through the granted handle. The returned path is an identity for
    /// editor bookkeeping; callers must consume the bytes without reopening it.
    pub fn read_path(&self, path: &Path) -> Result<(PathBuf, Vec<u8>), ServiceError> {
        self.require(Capability::WorkspaceRead)?;
        for (root, configured, directory) in &self.read_roots {
            let relative = if path.is_absolute() {
                let Ok(relative) = path
                    .strip_prefix(root)
                    .or_else(|_| path.strip_prefix(configured))
                else {
                    continue;
                };
                relative
            } else {
                path
            };
            let bytes = directory.read(relative).map_err(host_file_error)?;
            self.check_live()?;
            return Ok((root.join(relative), bytes));
        }
        Err(ServiceError::new(
            ErrorCode::PermissionDenied,
            "plugin path is outside the granted read roots",
        ))
    }

    pub fn read_root(&self, index: u32) -> Result<&ScopedDirectory, ServiceError> {
        self.require(Capability::WorkspaceRead)?;
        self.read_roots
            .get(index as usize)
            .map(|(_, _, dir)| dir)
            .ok_or_else(|| {
                ServiceError::new(
                    ErrorCode::PermissionDenied,
                    "plugin read root is not granted",
                )
            })
    }

    /// Identities captured when the granted directory handles were opened.
    /// Reporting them neither re-resolves aliases nor opens filesystem paths.
    pub fn read_roots(&self) -> Result<Vec<ReadRoot>, ServiceError> {
        self.require(Capability::WorkspaceRead)?;
        let mut bytes = 0usize;
        for (path, configured, _) in &self.read_roots {
            for path in [path, configured] {
                let path = path.to_str().ok_or_else(|| {
                    ServiceError::new(
                        ErrorCode::InvalidRequest,
                        "plugin read root metadata requires UTF-8 paths",
                    )
                })?;
                bytes = bytes
                    .checked_add(path.len())
                    .filter(|bytes| *bytes <= MAX_READ_ROOT_METADATA_BYTES)
                    .ok_or_else(|| {
                        ServiceError::new(
                            ErrorCode::ResourceExhausted,
                            "plugin read root metadata exceeds 64 KiB",
                        )
                    })?;
            }
        }
        let roots = self
            .read_roots
            .iter()
            .enumerate()
            .map(|(index, (path, configured, _))| ReadRoot {
                index: index as u32,
                path: path.to_str().unwrap().to_owned(),
                configured_path: configured.to_str().unwrap().to_owned(),
            })
            .collect();
        self.check_live()?;
        Ok(roots)
    }

    pub fn process_root(
        &self,
        index: u32,
        command: &str,
        args: &[String],
    ) -> Result<&Path, ServiceError> {
        self.check_live()?;
        self.permissions
            .require_process(&self.declared, command, args)?;
        self.read_roots
            .get(index as usize)
            .map(|(path, _, _)| path.as_path())
            .ok_or_else(|| {
                ServiceError::new(
                    ErrorCode::PermissionDenied,
                    "plugin process working directory is not granted",
                )
            })
    }

    pub fn write_root(&self, index: u32, path: &Path, bytes: &[u8]) -> Result<(), ServiceError> {
        self.require(Capability::WorkspaceWrite)?;
        let (_, _, directory) = self.write_roots.get(index as usize).ok_or_else(|| {
            ServiceError::new(
                ErrorCode::PermissionDenied,
                "plugin write root is not granted",
            )
        })?;
        directory.write(path, bytes).map_err(host_file_error)
    }
}

fn host_file_error(error: anyhow::Error) -> ServiceError {
    ServiceError::new(
        ErrorCode::PermissionDenied,
        format!("plugin file access denied: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn configured_alias_uses_the_original_handle_after_symlink_replacement() {
        let parent = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "allowed").unwrap();
        std::fs::write(outside.path().join("file"), "outside").unwrap();
        let alias = parent.path().join("alias");
        std::os::unix::fs::symlink(root.path(), &alias).unwrap();
        let policy = AccessPolicy::new(
            [Capability::WorkspaceRead].into(),
            Permissions {
                capabilities: [Capability::WorkspaceRead].into(),
                read_roots: vec![alias.clone()],
                ..Permissions::default()
            },
            Path::new("."),
        )
        .unwrap();
        let identities = vec![ReadRoot {
            index: 0,
            path: root.path().canonicalize().unwrap().to_str().unwrap().into(),
            configured_path: alias.to_str().unwrap().into(),
        }];
        assert_eq!(policy.read_roots().unwrap(), identities);
        assert_eq!(policy.read_path(&alias.join("file")).unwrap().1, b"allowed");
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(outside.path(), &alias).unwrap();
        assert_eq!(policy.read_roots().unwrap(), identities);
        assert_eq!(policy.read_path(&alias.join("file")).unwrap().1, b"allowed");
        assert!(policy.read_path(&outside.path().join("file")).is_err());
        policy.revoke();
        assert_eq!(policy.read_roots().unwrap_err().code, ErrorCode::Cancelled);
    }

    #[test]
    fn reads_are_scoped_and_revoked_handles_stop_work() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "content").unwrap();
        let permissions = Permissions {
            capabilities: [Capability::WorkspaceRead].into(),
            read_roots: vec![root.path().into()],
            ..Permissions::default()
        };
        let policy = AccessPolicy::new(
            [Capability::WorkspaceRead].into(),
            permissions,
            Path::new("."),
        )
        .unwrap();
        assert_eq!(policy.read_path(Path::new("file")).unwrap().1, b"content");
        assert!(policy.read_path(Path::new("../file")).is_err());
        policy.revoke();
        assert_eq!(
            policy.read_path(Path::new("file")).unwrap_err().code,
            ErrorCode::Cancelled
        );
    }
}
