use std::{
    ops::Range,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{ensure, Context, Result};
use gix::{
    bstr::{BStr, ByteSlice},
    ObjectId, Repository,
};

/// Include effective configuration and file contents in the blame cache key.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct IgnoredRevisions {
    files: Vec<(PathBuf, Vec<u8>)>,
    revisions: Vec<ObjectId>,
}

impl IgnoredRevisions {
    pub(super) fn read(repo: &Repository, workdir: &Path) -> Result<Self> {
        let config = repo.config_snapshot();
        let optional = config.string("blame.ignoreRevsFile").is_none();
        let mut files = Vec::new();
        for path in Self::paths(repo, workdir)? {
            match std::fs::read(&path) {
                Ok(contents) => files.push((path, contents)),
                Err(err) if optional && err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("Read ignored revisions from {}", path.display()))
                }
            }
        }
        let values = config
            .plumbing()
            .strings("blame.ignoreRevs")
            .unwrap_or_default();
        let mut revisions = Vec::new();
        for value in values
            .rsplit(|value| value.is_empty())
            .next()
            .unwrap_or_default()
        {
            // Cache the resolved commit, so moving an ignored tag cannot reuse stale blame.
            revisions.push(
                repo.rev_parse_single(value.as_ref())?
                    .object()?
                    .peel_to_commit()?
                    .id,
            );
        }
        Ok(Self { files, revisions })
    }

    pub(super) fn paths(repo: &Repository, workdir: &Path) -> Result<Vec<PathBuf>> {
        let config = repo.config_snapshot();
        let config = config.plumbing();
        let mut paths = Vec::new();
        if let Some(values) = config.strings("blame.ignoreRevsFile") {
            for value in values {
                if value.is_empty() {
                    paths.clear();
                } else {
                    let home = stdx::path::home_dir().ok();
                    let install = value
                        .starts_with(b"%(prefix)/")
                        .then(git_installation_prefix)
                        .transpose()?;
                    let path = gix::config::Path::from(value).interpolate(
                        gix::config::path::interpolate::Context {
                            home_dir: home.as_deref(),
                            git_install_dir: install.as_deref(),
                            home_for_user: Some(gix::config::path::interpolate::home_for_user),
                        },
                    )?;
                    paths.push(workdir.join(path));
                }
            }
        } else {
            // Watch the default even before it exists, so creating it refreshes blame.
            paths.push(workdir.join(".git-blame-ignore-revs"));
        }
        Ok(paths)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.files.is_empty() && self.revisions.is_empty()
    }

    pub(super) fn blame(
        &self,
        repo: &Repository,
        workdir: &Path,
        path: &BStr,
        head: ObjectId,
    ) -> Result<Vec<gix::blame::BlameEntry>> {
        // gitoxide cannot ignore revisions. Use Git's incremental format to retain
        // compact ranges and keep loading commit metadata through gitoxide.
        let mut command = Command::new("git");
        command
            .current_dir(dunce::simplified(workdir))
            .arg("--no-pager")
            .arg("--git-dir")
            .arg(dunce::simplified(repo.git_dir()))
            .arg("--work-tree")
            .arg(dunce::simplified(workdir))
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "blame.ignoreRevsFile=",
                "blame",
                "--incremental",
                "--no-textconv",
                "--no-ext-diff",
            ])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .stdin(Stdio::null());
        // Pin the discovered repository even when launched from another Git process.
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_PREFIX",
        ] {
            command.env_remove(name);
        }
        for (file, _) in &self.files {
            command
                .arg("--ignore-revs-file")
                .arg(dunce::simplified(file));
        }
        for revision in &self.revisions {
            command.arg("--ignore-rev").arg(revision.to_string());
        }
        let output = command
            .arg(head.to_string())
            .arg("--")
            .arg(gix::path::try_from_bstr(path)?.as_os_str())
            .output()
            .context("Run git blame with ignored revisions")?;
        ensure!(
            output.status.success(),
            "git blame failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        parse_entries(&output.stdout)
    }
}

fn git_installation_prefix() -> Result<PathBuf> {
    // gitoxide's install_dir() refers to the running Mitos binary, not Git.
    let output = Command::new("git")
        .args([
            "-c",
            "mitos.blamePrefix=%(prefix)/",
            "config",
            "--path",
            "--get",
            "mitos.blamePrefix",
        ])
        .stdin(Stdio::null())
        .output()
        .context("Resolve Git installation prefix")?;
    ensure!(
        output.status.success(),
        "Could not resolve Git installation prefix"
    );
    Ok(gix::path::try_from_byte_slice(output.stdout.trim_end())?.to_owned())
}

fn parse_entries(output: &[u8]) -> Result<Vec<gix::blame::BlameEntry>> {
    let mut lines = output.lines();
    let mut entries = Vec::new();
    while let Some(header) = lines.next() {
        let mut fields = header.split(|byte| *byte == b' ');
        let commit = ObjectId::from_hex(fields.next().context("Missing blame commit")?)?;
        let mut number = || -> Result<u32> {
            Ok(
                std::str::from_utf8(fields.next().context("Missing blame line number")?)?
                    .parse()?,
            )
        };
        let source = number()?;
        let result = number()?;
        let len = number()?;
        let range = |start: u32| -> Result<Range<u32>> {
            let start = start.checked_sub(1).context("Invalid blame line number")?;
            let end = start
                .checked_add(len)
                .context("Blame line range overflow")?;
            ensure!(len != 0, "Empty blame line range");
            Ok(start..end)
        };
        entries.push(gix::blame::BlameEntry::new(
            range(result)?,
            range(source)?,
            commit,
            None,
        ));
        // Metadata may contain arbitrary bytes. Only the filename terminator matters.
        ensure!(
            lines.by_ref().any(|line| line.starts_with(b"filename ")),
            "Incomplete blame entry"
        );
    }
    Ok(entries)
}
