use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Write,
    path::{Component, Path},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    thread,
};

use toml_edit::DocumentMut;

use crate::DynError;

struct Options {
    dry_run: bool,
    jobs: usize,
    grammars: HashSet<String>,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, DynError> {
        let mut options = Self {
            dry_run: false,
            jobs: 8,
            grammars: HashSet::new(),
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dry-run" => options.dry_run = true,
                "--jobs" => {
                    options.jobs = args
                        .next()
                        .ok_or("--jobs requires a count")?
                        .parse()
                        .map_err(|_| "--jobs must be a positive integer")?;
                    if options.jobs == 0 {
                        return Err("--jobs must be at least 1".into());
                    }
                }
                _ if arg.starts_with('-') => {
                    return Err(format!("unexpected argument: {arg}").into());
                }
                _ => {
                    options.grammars.insert(arg);
                }
            }
        }
        Ok(options)
    }
}

struct Grammar {
    index: usize,
    name: String,
    git: String,
    revision: String,
    parser: String,
}

enum Status {
    Current,
    Update(String),
    Skipped(String),
}

pub fn update(args: impl Iterator<Item = String>) -> Result<(), DynError> {
    update_at(&crate::path::project_root(), Options::parse(args)?)
}

fn update_at(root: &Path, options: Options) -> Result<(), DynError> {
    let path = root.join("languages.toml");
    let original = fs::read_to_string(&path)?;
    let mut document: DocumentMut = original.parse()?;
    let grammars = configured_grammars(&document, &options.grammars)?;

    // Several grammars can share a repository. Resolve and fetch it only once.
    let mut repositories = BTreeMap::<&str, Vec<&Grammar>>::new();
    for grammar in &grammars {
        repositories.entry(&grammar.git).or_default().push(grammar);
    }
    let repositories: Vec<_> = repositories.into_iter().collect();
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..options.jobs.min(repositories.len()) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some((remote, grammars)) = repositories.get(index) else {
                    break;
                };
                let inspected = inspect(remote, grammars);
                results.lock().unwrap().extend(inspected);
            });
        }
    });

    let mut results = results.into_inner().unwrap();
    results.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));
    let mut updated = 0;
    let mut skipped = 0;
    for (grammar, status) in results {
        match status {
            Status::Current => {}
            Status::Update(revision) => {
                println!(
                    "- `{}`: `{}` → `{revision}`",
                    grammar.name, grammar.revision
                );
                let source = document["grammar"]
                    .as_array_of_tables_mut()
                    .unwrap()
                    .get_mut(grammar.index)
                    .unwrap()["source"]
                    .as_table_like_mut()
                    .unwrap();
                let value = source.get_mut("rev").unwrap().as_value_mut().unwrap();
                let mut replacement = toml_edit::Value::from(revision);
                *replacement.decor_mut() = value.decor().clone();
                *value = replacement;
                updated += 1;
            }
            Status::Skipped(reason) => {
                println!("- Skipped `{}`: {reason}", grammar.name);
                skipped += 1;
            }
        }
    }
    println!(
        "\n{} {updated} grammar(s); {skipped} skipped.",
        if options.dry_run {
            "Would update"
        } else {
            "Updated"
        },
    );
    if updated > 0 && !options.dry_run {
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        temporary
            .as_file()
            .set_permissions(fs::metadata(&path)?.permissions())?;
        temporary.write_all(document.to_string().as_bytes())?;
        temporary.persist(&path)?;
    }
    Ok(())
}

fn configured_grammars(
    document: &DocumentMut,
    selected: &HashSet<String>,
) -> Result<Vec<Grammar>, DynError> {
    let entries = document
        .get("grammar")
        .and_then(|item| item.as_array_of_tables())
        .ok_or("languages.toml has no grammar entries")?;
    let mut names = HashSet::new();
    let mut grammars = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(|item| item.as_str())
            .ok_or("grammar has no name")?;
        if !names.insert(name) {
            return Err(format!("duplicate grammar: {name}").into());
        }
        if !selected.is_empty() && !selected.contains(name) {
            continue;
        }
        let source = entry
            .get("source")
            .and_then(|item| item.as_table_like())
            .ok_or_else(|| format!("{name}: missing grammar source"))?;
        if source.contains_key("path") {
            continue;
        }
        let git = source.get("git").and_then(|item| item.as_str());
        let revision = source.get("rev").and_then(|item| item.as_str());
        let (Some(git), Some(revision)) = (git, revision) else {
            return Err(format!("{name}: source requires git and rev").into());
        };
        let subpath = source
            .get("subpath")
            .map(|item| item.as_str().ok_or("subpath must be a string"))
            .transpose()?
            .unwrap_or("");
        let mut parser = Vec::new();
        for component in Path::new(subpath).components() {
            match component {
                Component::Normal(part) => parser.push(part.to_string_lossy().into_owned()),
                Component::CurDir => {}
                _ => return Err(format!("{name}: subpath must stay within the repository").into()),
            }
        }
        parser.push("src/parser.c".into());
        grammars.push(Grammar {
            index,
            name: name.into(),
            git: git.into(),
            revision: revision.into(),
            parser: parser.join("/"),
        });
    }
    for name in selected {
        if !names.contains(name.as_str()) {
            return Err(format!("unknown grammar: {name}").into());
        }
    }
    Ok(grammars)
}

fn inspect<'a>(remote: &str, grammars: &[&'a Grammar]) -> Vec<(&'a Grammar, Status)> {
    let result = (|| -> Result<_, DynError> {
        let directory = tempfile::tempdir()?;
        let probe = init_repository(&directory.path().join("refs"), gix::hash::Kind::Sha1)?;
        let revision = remote_head(&probe, remote)?;
        if grammars
            .iter()
            .all(|g| current_revision(&g.revision) == Some(revision))
        {
            return Ok((revision, None));
        }
        let repo = init_repository(&directory.path().join("objects"), revision.kind())?;
        let refspec = revision.to_string();
        let upstream = repo
            .remote_at(remote)?
            .with_refspecs([refspec.as_str()], gix::remote::Direction::Fetch)?
            .with_fetch_tags(gix::remote::fetch::Tags::None);
        upstream
            .connect(gix::remote::Direction::Fetch)?
            .prepare_fetch(gix::progress::Discard, Default::default())?
            .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(1.try_into()?))
            .receive(gix::progress::Discard, &AtomicBool::new(false))?;
        // Close object database handles before removing the temporary directory.
        Ok((revision, Some((repo, directory))))
    })()
    .map_err(|error| format!("could not inspect {remote}: {error}"));
    grammars
        .iter()
        .map(|&grammar| {
            let status = match &result {
                Err(error) => Status::Skipped(error.clone()),
                Ok((revision, _)) if current_revision(&grammar.revision) == Some(*revision) => {
                    Status::Current
                }
                Ok((revision, Some((repo, _directory)))) => {
                    match committed_parser(repo, *revision, &grammar.parser) {
                        Ok(true) => Status::Update(revision.to_string()),
                        Ok(false) => Status::Skipped(format!(
                            "upstream HEAD has no committed `{}`",
                            grammar.parser
                        )),
                        Err(error) => Status::Skipped(format!(
                            "could not inspect upstream `{}`: {error}",
                            grammar.parser
                        )),
                    }
                }
                Ok((_, None)) => unreachable!(),
            };
            (grammar, status)
        })
        .collect()
}

fn init_repository(path: &Path, object_hash: gix::hash::Kind) -> Result<gix::Repository, DynError> {
    Ok(gix::ThreadSafeRepository::init_opts(
        path,
        gix::create::Kind::Bare,
        gix::create::Options {
            object_hash: Some(object_hash),
            ..Default::default()
        },
        gix::open::Options::default().config_overrides([
            "http.lowSpeedLimit=1024",
            "http.lowSpeedTime=60",
            "gitoxide.credentials.terminalPrompt=false",
        ]),
    )?
    .to_thread_local())
}

fn remote_head(repo: &gix::Repository, url: &str) -> Result<gix::ObjectId, DynError> {
    let remote = repo
        .remote_at(url)?
        .with_refspecs(["HEAD"], gix::remote::Direction::Fetch)?
        .with_fetch_tags(gix::remote::fetch::Tags::None);
    let (refs, _) = remote
        .connect(gix::remote::Direction::Fetch)?
        .ref_map(gix::progress::Discard, Default::default())?;
    refs.mappings
        .iter()
        .find_map(|mapping| mapping.remote.as_id().map(ToOwned::to_owned))
        .ok_or_else(|| "upstream did not advertise a HEAD revision".into())
}

fn committed_parser(
    repo: &gix::Repository,
    revision: gix::ObjectId,
    path: &str,
) -> Result<bool, DynError> {
    let tree = repo.find_commit(revision)?.tree()?;
    let Some(entry) = tree.lookup_entry_by_path(path)? else {
        return Ok(false);
    };
    Ok(entry.object()?.kind == gix::objs::Kind::Blob)
}

fn current_revision(revision: &str) -> Option<gix::ObjectId> {
    let revision = revision
        .strip_prefix("sha1:")
        .or_else(|| revision.strip_prefix("sha256:"))
        .unwrap_or(revision);
    gix::ObjectId::from_hex(revision.as_bytes()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    // Use Git to create independent fixtures for the gix implementation.
    fn git(directory: Option<&Path>, args: &[&str]) -> Result<String, String> {
        let mut command = Command::new("git");
        command
            .args(["-c", "core.fsmonitor=false"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null());
        if let Some(directory) = directory {
            command.current_dir(directory);
        }
        let output = command.output().map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "git {}: {}",
                args[0],
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    }

    fn options(args: &[&str]) -> Options {
        Options::parse(args.iter().map(|arg| (*arg).to_string())).unwrap()
    }

    fn commit(repo: &Path) -> String {
        git(Some(repo), &["add", "--all"]).unwrap();
        git(
            Some(repo),
            &[
                "-c",
                "user.name=Grammar update tests",
                "-c",
                "user.email=grammar-tests@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        git(Some(repo), &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .into()
    }

    fn repository() -> (tempfile::TempDir, String, String) {
        repository_with_format("sha1")
    }

    fn repository_with_format(format: &str) -> (tempfile::TempDir, String, String) {
        let repo = tempfile::tempdir().unwrap();
        git(
            Some(repo.path()),
            &["init", "--quiet", "-b", "main", "--object-format", format],
        )
        .unwrap();
        fs::create_dir_all(repo.path().join("src")).unwrap();
        fs::write(repo.path().join("src/parser.c"), "old parser\n").unwrap();
        let old = commit(repo.path());
        fs::write(repo.path().join("src/parser.c"), "new parser\n").unwrap();
        fs::create_dir_all(repo.path().join("nested/src")).unwrap();
        fs::write(repo.path().join("nested/src/parser.c"), "nested parser\n").unwrap();
        let new = commit(repo.path());
        (repo, old, new)
    }

    fn manifest(repo: &Path, revision: &str) -> String {
        format!(
            r#"# Preserve this comment and the interleaved language entry.
[[grammar]]
name = "alpha"
source = {{ git = {repo:?}, rev = "{revision}" }} # pinned

[[language]]
name = "beta"
file-types = ["beta"]

[[grammar]]
name = "beta"
source = {{ git = {repo:?}, subpath = "nested", rev = "{revision}" }}

[[grammar]]
name = "missing"
source = {{ git = {repo:?}, rev = "{revision}", subpath = "removed" }}

[[grammar]]
name = "local"
source = {{ path = "/local/grammar" }}
"#,
        )
    }

    #[test]
    fn updates_shared_repository_and_subpaths_without_reformatting() {
        let (repo, old, new) = repository();
        let root = tempfile::tempdir().unwrap();
        let original = manifest(repo.path(), &old);
        fs::write(root.path().join("languages.toml"), &original).unwrap();
        update_at(root.path(), options(&["--jobs", "2"])).unwrap();
        // Only the first two revision values change. Missing parsers stay pinned.
        assert_eq!(
            fs::read_to_string(root.path().join("languages.toml")).unwrap(),
            original.replacen(&old, &new, 2),
        );
    }

    #[test]
    fn dry_run_does_not_write_and_filter_limits_mutation() {
        let (repo, old, new) = repository();
        let root = tempfile::tempdir().unwrap();
        let original = manifest(repo.path(), &old);
        let path = root.path().join("languages.toml");
        fs::write(&path, &original).unwrap();
        update_at(root.path(), options(&["--dry-run"])).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        update_at(root.path(), options(&["beta"])).unwrap();
        let mut expected = original.clone();
        let beta = expected.find("subpath = \"nested\"").unwrap();
        expected.replace_range(beta.., &original[beta..].replacen(&old, &new, 1));
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    }

    #[test]
    fn removed_parser_keeps_the_existing_pin() {
        let (repo, old, _) = repository();
        fs::remove_file(repo.path().join("src/parser.c")).unwrap();
        commit(repo.path());
        let root = tempfile::tempdir().unwrap();
        let original = manifest(repo.path(), &old);
        let path = root.path().join("languages.toml");
        fs::write(&path, &original).unwrap();
        update_at(root.path(), options(&["alpha"])).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn parser_directory_keeps_the_existing_pin() {
        let (repo, old, _) = repository();
        fs::remove_file(repo.path().join("src/parser.c")).unwrap();
        fs::create_dir(repo.path().join("src/parser.c")).unwrap();
        fs::write(repo.path().join("src/parser.c/placeholder"), "not a parser").unwrap();
        commit(repo.path());
        let root = tempfile::tempdir().unwrap();
        let original = manifest(repo.path(), &old);
        let path = root.path().join("languages.toml");
        fs::write(&path, &original).unwrap();
        update_at(root.path(), options(&["alpha"])).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn detached_upstream_head_is_updated() {
        let (repo, old, new) = repository();
        git(
            Some(repo.path()),
            &["checkout", "--quiet", "--detach", &new],
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let original = manifest(repo.path(), &old);
        let path = root.path().join("languages.toml");
        fs::write(&path, &original).unwrap();
        update_at(root.path(), options(&["alpha"])).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            original.replacen(&old, &new, 1),
        );
    }

    #[test]
    fn current_and_unreachable_sources_leave_the_file_unchanged() {
        let (repo, _, new) = repository();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("languages.toml");
        let current = manifest(repo.path(), &format!("sha1:{new}"));
        fs::write(&path, &current).unwrap();
        update_at(root.path(), options(&[])).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), current);

        let unreachable = manifest(&root.path().join("absent"), &new);
        fs::write(&path, &unreachable).unwrap();
        update_at(root.path(), options(&[])).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), unreachable);
    }

    #[test]
    fn invalid_selection_and_subpaths_do_not_mutate_the_manifest() {
        let (repo, old, _) = repository();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("languages.toml");
        let original = manifest(repo.path(), &old);
        fs::write(&path, &original).unwrap();
        assert!(update_at(root.path(), options(&["typo"])).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        let invalid = original.replace("subpath = \"nested\"", "subpath = \"../outside\"");
        fs::write(&path, &invalid).unwrap();
        assert!(update_at(root.path(), options(&[])).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    }

    #[test]
    fn rejects_invalid_options() {
        for args in [
            vec!["--jobs"],
            vec!["--jobs", "0"],
            vec!["--jobs", "many"],
            vec!["--unknown"],
        ] {
            assert!(Options::parse(args.into_iter().map(String::from)).is_err());
        }
    }

    #[test]
    fn updates_sha256_repository_and_recognizes_prefixed_pin() {
        let (repo, old, new) = repository_with_format("sha256");
        let root = tempfile::tempdir().unwrap();
        let original = manifest(repo.path(), &format!("sha256:{old}"));
        let path = root.path().join("languages.toml");
        fs::write(&path, &original).unwrap();
        update_at(root.path(), options(&["alpha"])).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            original.replacen(&format!("sha256:{old}"), &new, 1),
        );
        let current = manifest(repo.path(), &format!("sha256:{new}"));
        fs::write(&path, &current).unwrap();
        update_at(root.path(), options(&[])).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), current);
    }

    #[test]
    fn unborn_head_keeps_the_existing_pin() {
        let repo = tempfile::tempdir().unwrap();
        git(Some(repo.path()), &["init", "--quiet", "-b", "main"]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let original = manifest(repo.path(), &"0".repeat(40));
        let path = root.path().join("languages.toml");
        fs::write(&path, &original).unwrap();
        update_at(root.path(), options(&[])).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }
}
