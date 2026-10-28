//! Read-only, commit-pinned links. Remote configuration is data, never a command.

use std::{
    ops::RangeInclusive,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use gix::bstr::ByteSlice;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

use crate::operations::history::RevisionId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Github,
    Gitlab,
}

#[derive(Clone, Debug)]
pub struct Remote {
    pub name: String,
    base: String,
    provider: Provider,
}

/// Materialized on a worker; the object ID and bytes always describe the same commit.
pub struct FileRevision {
    pub revision: String,
    pub path: PathBuf,
    /// Hosting line numbers refer to raw committed content, never smudge-filter output.
    pub bytes: Vec<u8>,
    pub remotes: Vec<Remote>,
}

impl FileRevision {
    pub fn url(
        &self,
        lines: Option<RangeInclusive<usize>>,
        remote: Option<&str>,
    ) -> Result<String> {
        let remote = choose_remote(&self.remotes, remote)?;
        let path = self
            .path
            .components()
            .map(|part| {
                part.as_os_str()
                    .to_str()
                    .context("Non-UTF-8 paths cannot be linked")
                    .map(encode)
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        let separator = match remote.provider {
            Provider::Github => "blob",
            Provider::Gitlab => "-/blob",
        };
        let mut url = format!("{}/{separator}/{}/{path}", remote.base, self.revision);
        if let Some(lines) = lines {
            ensure!(
                lines.start() <= lines.end(),
                "Expected an ordered line range"
            );
            let start = lines
                .start()
                .checked_add(1)
                .context("Line number is too large")?;
            let end = lines
                .end()
                .checked_add(1)
                .context("Line number is too large")?;
            url.push_str(&if start == end {
                format!("#L{start}")
            } else {
                match remote.provider {
                    Provider::Github => format!("#L{start}-L{end}"),
                    Provider::Gitlab => format!("#L{start}-{end}"),
                }
            });
        }
        Ok(url)
    }
}

fn encode(value: &str) -> String {
    const SAFE: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    utf8_percent_encode(value, SAFE).to_string()
}

fn parse_remote(name: &str, url: &str, provider: Option<&str>) -> Result<Remote> {
    // Do not propagate parser errors, which can contain credentials from the input URL.
    let url = gix::url::parse(url.as_bytes().as_bstr())
        .map_err(|_| anyhow::anyhow!("Unsupported remote URL; expected SSH or HTTPS"))?;
    ensure!(
        matches!(
            url.scheme,
            gix::url::Scheme::Ssh | gix::url::Scheme::Http | gix::url::Scheme::Https
        ),
        "Unsupported remote URL; expected SSH or HTTPS"
    );
    let host = url.host.as_deref().context("Remote URL has no host")?;
    ensure!(
        !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')),
        "Unsupported remote host"
    );
    let provider = match provider {
        Some("github") => Provider::Github,
        Some("gitlab") => Provider::Gitlab,
        Some(_) => bail!("Unknown permalink provider; expected github or gitlab"),
        None if host.eq_ignore_ascii_case("github.com")
            || host.to_ascii_lowercase().contains("github") =>
        {
            Provider::Github
        }
        None if host.eq_ignore_ascii_case("gitlab.com")
            || host.to_ascii_lowercase().contains("gitlab") =>
        {
            Provider::Gitlab
        }
        None => {
            bail!("Unknown hosting provider; set remote.{name}.mitos-provider to github or gitlab")
        }
    };
    let path = url.path.to_str().context("Remote path is not UTF-8")?;
    let path = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_matches('/'));
    let parts = path
        .split('/')
        .map(|part| {
            ensure!(
                !part.is_empty() && !matches!(part, "." | "..") && !part.contains(['\\', '\0']),
                "Invalid remote repository path"
            );
            Ok(encode(part))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        parts.len() >= 2 && (provider == Provider::Gitlab || parts.len() == 2),
        "Invalid repository path for hosting provider"
    );
    Ok(Remote {
        name: name.into(),
        // Transport credentials and SSH ports never belong in the web link.
        base: format!(
            "https://{host}{}/{}",
            if url.scheme == gix::url::Scheme::Ssh {
                String::new()
            } else {
                url.port.map_or_else(String::new, |port| format!(":{port}"))
            },
            parts.join("/")
        ),
        provider,
    })
}

fn remotes(repo: &gix::Repository) -> Result<Vec<Remote>> {
    let snapshot = repo.config_snapshot();
    let config = snapshot.plumbing();
    let mut names = config
        .sections_by_name("remote")
        .into_iter()
        .flatten()
        .filter_map(|section| {
            section
                .header()
                .subsection_name()
                .map(|name| name.to_string())
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    let mut result = Vec::new();
    let mut failure = None;
    for name in names {
        let Some(url) = config.string(&format!("remote.{name}.url")) else {
            continue;
        };
        let provider = config.string(&format!("remote.{name}.mitos-provider"));
        match parse_remote(
            &name,
            url.to_str()?,
            provider.as_ref().map(|p| p.to_str()).transpose()?,
        ) {
            Ok(remote) => result.push(remote),
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    if result.is_empty() {
        return Err(
            failure.unwrap_or_else(|| anyhow::anyhow!("No supported Git remote configured"))
        );
    }
    Ok(result)
}

fn choose_remote<'a>(remotes: &'a [Remote], selected: Option<&str>) -> Result<&'a Remote> {
    if let Some(selected) = selected {
        return remotes
            .iter()
            .find(|remote| remote.name == selected)
            .context("Selected remote is unavailable or has an unsupported provider");
    }
    ensure!(
        remotes.len() == 1,
        "Choose a remote by appending its name to the command: {}",
        remotes
            .iter()
            .map(|remote| remote.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    remotes
        .first()
        .context("No supported Git remote configured")
}

pub fn file_revision(path: &Path, revision: Option<&str>, trusted: bool) -> Result<FileRevision> {
    let directory = path
        .parent()
        .context("File has no parent")?
        .ancestors()
        .find(|path| path.is_dir())
        .context("Repository no longer exists")?;
    let repo = crate::git::open_repo(directory, trusted)?.to_thread_local();
    let root = repo.workdir().context("Repository has no working tree")?;
    let relative = path.strip_prefix(root)?.to_owned();
    let id = match revision {
        Some(id) => gix::ObjectId::from_hex(RevisionId::parse(id)?.as_str().as_bytes())?,
        None => {
            repo.head_commit()
                .context("Create a commit before copying a permalink")?
                .id
        }
    };
    let commit = repo.find_commit(id)?;
    let tree = commit.tree()?;
    let entry = tree
        .lookup_entry_by_path(&relative)?
        .context("This file does not exist at the linked commit")?;
    ensure!(
        matches!(
            entry.mode().kind(),
            gix::objs::tree::EntryKind::Blob | gix::objs::tree::EntryKind::BlobExecutable
        ),
        "Permalinks require a regular text file"
    );
    ensure!(
        repo.find_header(entry.object_id())?.size() <= crate::MAX_REVIEW_BYTES as u64,
        "Committed file too large to map permalink lines"
    );
    let bytes = repo.find_blob(entry.object_id())?.data.to_vec();
    Ok(FileRevision {
        revision: id.to_string(),
        path: relative,
        bytes,
        remotes: remotes(&repo)?,
    })
}

pub fn commit_url(
    path: &Path,
    revision: &str,
    selected: Option<&str>,
    trusted: bool,
) -> Result<String> {
    RevisionId::parse(revision)?;
    let directory = path
        .ancestors()
        .find(|path| path.is_dir())
        .context("Repository no longer exists")?;
    let repo = crate::git::open_repo(directory, trusted)?.to_thread_local();
    let remotes = remotes(&repo)?;
    let remote = choose_remote(&remotes, selected)?;
    let separator = match remote.provider {
        Provider::Github => "commit",
        Provider::Gitlab => "-/commit",
    };
    Ok(format!("{}/{separator}/{revision}", remote.base))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(remote: Remote) -> FileRevision {
        FileRevision {
            revision: "a".repeat(40),
            path: "src/a #ü.rs".into(),
            bytes: Vec::new(),
            remotes: vec![remote],
        }
    }

    #[test]
    fn hosts_credentials_ports_escaping_and_line_ranges() -> Result<()> {
        let github = file(parse_remote(
            "origin",
            "git@github.com:owner/repo.git",
            None,
        )?);
        assert_eq!(
            github.url(Some(2..=5), None)?,
            format!(
                "https://github.com/owner/repo/blob/{}/src/a%20%23%C3%BC.rs#L3-L6",
                "a".repeat(40)
            )
        );
        let gitlab = file(parse_remote(
            "origin",
            "ssh://git@gitlab.example.com:2222/group/nested/repo.git",
            None,
        )?);
        assert!(gitlab.url(Some(2..=5), None)?.ends_with("#L3-6"));
        assert!(gitlab
            .url(None, None)?
            .starts_with("https://gitlab.example.com/group/nested/repo/-/blob/"));
        let private = file(parse_remote(
            "origin",
            "https://user:password@code.example.com:8443/team/repo.git",
            Some("gitlab"),
        )?);
        assert!(private
            .url(None, None)?
            .starts_with("https://code.example.com:8443/team/repo/"));
        assert!(!private.url(None, None)?.contains("password"));
        assert!(parse_remote("origin", "file:///tmp/repo", None).is_err());
        assert!(parse_remote("origin", "https://github.com/team/../repo", None).is_err());
        Ok(())
    }

    #[test]
    fn multiple_remotes_require_an_explicit_choice() -> Result<()> {
        let mut file = file(parse_remote(
            "origin",
            "git@github.com:team/repo.git",
            None,
        )?);
        file.remotes.push(parse_remote(
            "upstream",
            "https://gitlab.com/team/repo",
            None,
        )?);
        assert!(file.url(None, None).is_err());
        assert!(file.url(Some(0..=0), Some("upstream"))?.ends_with("#L1"));
        assert!(file.url(None, Some("missing")).is_err());
        Ok(())
    }

    #[test]
    fn repository_links_pin_commit_content_and_read_remotes_without_executing_config() -> Result<()>
    {
        use crate::git::test::{create_commit_with_message, empty_git_repo, exec_git_cmd};
        let dir = empty_git_repo();
        let path = dir.path().join("a #ü.txt");
        std::fs::write(dir.path().join(".gitattributes"), "* text eol=lf\n")?;
        std::fs::write(&path, "initial\n")?;
        create_commit_with_message(dir.path(), true, "initial");
        exec_git_cmd(
            &["remote", "add", "origin", "git@github.com:owner/repo.git"],
            dir.path(),
        );
        let first = file_revision(&path, None, false)?;
        assert_eq!(first.bytes, b"initial\n");
        std::fs::write(&path, "later\n")?;
        create_commit_with_message(dir.path(), true, "later");
        let historical = file_revision(&path, Some(&first.revision), false)?;
        assert_eq!(historical.bytes, b"initial\n");
        assert!(historical.url(Some(0..=0), None)?.contains(&first.revision));
        assert!(commit_url(dir.path(), &first.revision, None, false)?
            .ends_with(&format!("/commit/{}", first.revision)));
        assert_eq!(
            commit_url(
                &dir.path().join("deleted/nested/file.txt"),
                &first.revision,
                None,
                false
            )?,
            commit_url(dir.path(), &first.revision, None, false)?
        );
        assert!(file_revision(&dir.path().join("untracked.txt"), None, false).is_err());
        std::fs::write(
            dir.path().join(".gitattributes"),
            "* text eol=lf filter=unavailable\n",
        )?;
        exec_git_cmd(
            &[
                "config",
                "filter.unavailable.smudge",
                "mitos-test-command-that-does-not-exist",
            ],
            dir.path(),
        );
        assert_eq!(
            file_revision(&path, Some(&first.revision), true)?.bytes,
            b"initial\n"
        );
        Ok(())
    }
}
