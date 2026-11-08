use std::{
    io::{BufRead, BufReader, Read},
    ops::Range,
    path::{Path, PathBuf},
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
                repo.rev_parse_single(value.as_bstr())?
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
        let mut command =
            crate::command::repository(workdir, repo.git_dir(), crate::command::Access::ReadOnly);
        command.args([
            "-c",
            "blame.ignoreRevsFile=",
            "blame",
            "--incremental",
            "--no-textconv",
            "--no-ext-diff",
        ]);
        for (file, _) in &self.files {
            command
                .arg("--ignore-revs-file")
                .arg(dunce::simplified(file));
        }
        for revision in &self.revisions {
            command.arg("--ignore-rev").arg(revision.to_string());
        }
        command
            .arg(head.to_string())
            .arg("--")
            .arg(gix::path::from_bstr(path)?.as_os_str());
        crate::command::read_sync(command, parse_entries)
            .context("Run git blame with ignored revisions")
    }
}

fn git_installation_prefix() -> Result<PathBuf> {
    // gitoxide's install_dir() refers to the running Mitos binary, not Git.
    let mut command = crate::command::system(crate::command::Access::ReadOnly);
    command.args([
        "-c",
        "mitos.blamePrefix=%(prefix)/",
        "config",
        "--path",
        "--get",
        "mitos.blamePrefix",
    ]);
    let output = crate::command::run_sync(command, crate::command::Output::Display)
        .context("Resolve Git installation prefix")?;
    Ok(gix::path::from_byte_slice(output.trim_end())?.to_owned())
}

// Headers contain only an object ID and three numbers. Metadata (including a
// potentially huge commit summary) needs only a prefix to find the filename marker.
const PROTOCOL_LINE_PREFIX: usize = 1024;

fn read_protocol_line(reader: &mut dyn BufRead, line: &mut Vec<u8>) -> Result<bool> {
    line.clear();
    if (&mut *reader)
        .take((PROTOCOL_LINE_PREFIX + 1) as u64)
        .read_until(b'\n', line)?
        == 0
    {
        return Ok(false);
    }
    if line.ends_with(b"\n") {
        line.pop();
        if line.ends_with(b"\r") {
            line.pop();
        }
    } else if line.len() > PROTOCOL_LINE_PREFIX {
        reader.skip_until(b'\n')?;
    }
    Ok(true)
}

fn parse_entries(output: &mut dyn Read) -> Result<Vec<gix::blame::BlameEntry>> {
    let mut reader = BufReader::new(output);
    let mut line = Vec::new();
    let mut entries = Vec::new();
    while read_protocol_line(&mut reader, &mut line)? {
        ensure!(
            line.len() <= PROTOCOL_LINE_PREFIX,
            "Blame header exceeds the protocol limit"
        );
        let mut fields = line.split(|byte| *byte == b' ');
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
        loop {
            ensure!(
                read_protocol_line(&mut reader, &mut line)?,
                "Incomplete blame entry"
            );
            if line.starts_with(b"filename ") {
                break;
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_parser_skips_large_metadata_with_a_bounded_line_prefix() -> Result<()> {
        let id = "a".repeat(40);
        let mut output = std::io::Cursor::new(format!("{id} 1 1 1\nsummary "))
            .chain(std::io::repeat(b'x').take(2 * 1024 * 1024))
            .chain(std::io::Cursor::new(format!(
                "\nfilename file\n{id} 2 2 1\nfilename file\n"
            )));
        assert_eq!(parse_entries(&mut output)?.len(), 2);
        Ok(())
    }
}
