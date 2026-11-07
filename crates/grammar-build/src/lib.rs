//! Fetch and compile explicitly supplied grammar definitions into a runtime directory.
//!
//! Callers resolve configuration and workspace trust before invoking this crate.
//! Loading compiled grammars and runtime resources belongs to `loader`.

use anyhow::{anyhow, bail, Context, Result};
use loader::grammar::{GrammarConfiguration, GrammarSource, DYLIB_EXTENSION};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::channel,
        Arc,
    },
    time::SystemTime,
};
use tempfile::TempPath;

const BUILD_TARGET: &str = env!("BUILD_TARGET");

pub fn fetch_grammars(
    grammars: Vec<GrammarConfiguration>,
    runtime_directory: &Path,
    strict: bool,
) -> Result<()> {
    // We do not need to fetch local grammars.
    let mut grammars = grammars;
    let runtime_directory = runtime_directory.to_owned();
    grammars.retain(|grammar| !matches!(grammar.source, GrammarSource::Local { .. }));

    let results = run_parallel(grammars, "Fetching", move |grammar| {
        fetch_grammar(grammar, &runtime_directory)
    });

    let mut errors = Vec::new();
    let mut git_updated = Vec::new();
    let mut git_up_to_date = 0;

    for (grammar_id, res) in results {
        match res {
            Ok(FetchStatus::GitUpToDate) => git_up_to_date += 1,
            Ok(FetchStatus::GitUpdated { revision }) => git_updated.push((grammar_id, revision)),
            Err(e) => errors.push((grammar_id, e)),
        }
    }

    git_updated.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    if git_up_to_date != 0 {
        println!("{} up to date git grammars", git_up_to_date);
    }

    if !git_updated.is_empty() {
        println!("{} updated grammars", git_updated.len());
        // We checked the vec is not empty, unwrapping will not panic
        let longest_id = git_updated.iter().map(|x| x.0.len()).max().unwrap();
        for (id, rev) in git_updated {
            println!(
                "\t{id:width$} now on {rev}",
                id = id,
                width = longest_id,
                rev = rev
            );
        }
    }

    report_failures(errors, strict, "fetch")
}

pub fn build_grammars(
    grammars: Vec<GrammarConfiguration>,
    runtime_directory: &Path,
    target: Option<String>,
    strict: bool,
) -> Result<()> {
    let runtime_directory = runtime_directory.to_owned();
    let results = run_parallel(grammars, "Building", move |grammar| {
        build_grammar(grammar, &runtime_directory, target.as_deref())
    });

    let mut errors = Vec::new();
    let mut already_built = 0;
    let mut built = Vec::new();

    for (grammar_id, res) in results {
        match res {
            Ok(BuildStatus::AlreadyBuilt) => already_built += 1,
            Ok(BuildStatus::Built) => built.push(grammar_id),
            Err(e) => errors.push((grammar_id, e)),
        }
    }

    built.sort_unstable();

    if already_built != 0 {
        println!("{} grammars already built", already_built);
    }

    if !built.is_empty() {
        println!("{} grammars built now", built.len());
        println!("\t{:?}", built);
    }

    report_failures(errors, strict, "build")
}

fn report_failures(errors: Vec<(String, anyhow::Error)>, strict: bool, action: &str) -> Result<()> {
    let len = errors.len();
    for (i, (grammar, error)) in errors.into_iter().enumerate() {
        println!("Failure {}/{len}: {grammar} {error}", i + 1);
    }
    if strict && len != 0 {
        bail!("{len} grammars failed to {action}");
    }
    Ok(())
}

fn run_parallel<F, Res>(
    grammars: Vec<GrammarConfiguration>,
    action: &'static str,
    job: F,
) -> Vec<(String, Result<Res>)>
where
    F: Fn(GrammarConfiguration) -> Result<Res> + Send + 'static + Clone,
    Res: Send + 'static,
{
    let pool = threadpool::Builder::new().build();
    let (tx, rx) = channel();
    let total = grammars.len();
    let counter = Arc::new(AtomicUsize::new(0));
    println!("{action} {total} grammars");

    for grammar in grammars {
        let tx = tx.clone();
        let job = job.clone();
        let counter = Arc::clone(&counter);

        pool.execute(move || {
            let current = counter.fetch_add(1, Ordering::Relaxed) + 1;
            println!(
                "{action} grammars ({current}/{total}): {}",
                grammar.grammar_id
            );
            // Ignore any SendErrors, if any job in another thread has encountered an
            // error the Receiver will be closed causing this send to fail.
            let _ = tx.send((grammar.grammar_id.clone(), job(grammar)));
        });
    }

    drop(tx);

    rx.iter().collect()
}

enum FetchStatus {
    GitUpToDate,
    GitUpdated { revision: String },
}

#[derive(Copy, Clone)]
enum GitObjectFormat {
    Sha1,
    Sha256,
}

impl GitObjectFormat {
    fn kind(self) -> gix::hash::Kind {
        match self {
            Self::Sha1 => gix::hash::Kind::Sha1,
            Self::Sha256 => gix::hash::Kind::Sha256,
        }
    }
}

fn extract_object_format_from_revision(rev: &str) -> (GitObjectFormat, &str) {
    if let Some(stripped) = rev.strip_prefix("sha1:") {
        return (GitObjectFormat::Sha1, stripped);
    }

    if let Some(stripped) = rev.strip_prefix("sha256:") {
        return (GitObjectFormat::Sha256, stripped);
    }

    if rev.len() == 64 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        return (GitObjectFormat::Sha256, rev);
    }

    (GitObjectFormat::Sha1, rev)
}

struct VendoredGrammar {
    dir: PathBuf,
}

impl VendoredGrammar {
    fn new(grammar: &str, runtime_directory: &Path) -> Self {
        let dir = runtime_directory
            .join("grammars")
            .join("sources")
            .join(grammar);

        Self { dir }
    }

    fn revision(&self) -> Option<String> {
        gix::open(&self.dir)
            .ok()?
            .head_id()
            .ok()
            .map(|id| id.to_string())
    }

    /// Grammar sources are disposable: populate a fresh worktree at the pinned revision.
    fn fetch(&self, remote: &str, rev: &str, object_format: GitObjectFormat) -> Result<()> {
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::create_dir_all(&self.dir)?;
        let mut repo = gix::ThreadSafeRepository::init_opts(
            &self.dir,
            gix::create::Kind::WithWorktree,
            gix::create::Options {
                object_hash: Some(object_format.kind()),
                ..Default::default()
            },
            gix::open::Options::default(),
        )?
        .to_thread_local();
        repo.committer_or_set_generic_fallback()?;
        let refspec = format!("{rev}:refs/mitos/grammar");
        let mut upstream = repo.remote_at(remote)?;
        let mut url = upstream
            .url(gix::remote::Direction::Fetch)
            .context("Grammar remote has no URL")?
            .clone();
        if url.scheme == gix::url::Scheme::File {
            let path = gix::path::from_bstr(&url.path);
            if !path.is_absolute() {
                url.path = gix::path::into_bstr(self.dir.join(path)).into_owned();
                upstream = upstream.with_url_without_url_rewrite(url)?;
            }
        }
        upstream
            .with_refspecs([refspec.as_str()], gix::remote::Direction::Fetch)?
            .with_fetch_tags(gix::remote::fetch::Tags::None)
            .connect(gix::remote::Direction::Fetch)?
            .prepare_fetch(gix::progress::Discard, Default::default())?
            .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(1.try_into()?))
            .receive(gix::progress::Discard, &AtomicBool::new(false))?;
        let commit = repo
            .find_reference("refs/mitos/grammar")?
            .peel_to_commit()?;
        let mut index = repo.index_from_tree(&commit.tree_id()?)?;
        let mut options =
            repo.checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)?;
        options.destination_is_initially_empty = true;
        let outcome = gix::worktree::state::checkout(
            &mut index,
            repo.workdir()
                .context("grammar repository has no worktree")?,
            repo.objects.clone().into_arc()?,
            &gix::progress::Discard,
            &gix::progress::Discard,
            &AtomicBool::new(false),
            options,
        )?;
        if !outcome.errors.is_empty() || !outcome.collisions.is_empty() {
            bail!("Could not check out grammar files: {outcome:?}");
        }
        index.write(Default::default())?;
        repo.reference(
            "HEAD",
            commit.id,
            gix::refs::transaction::PreviousValue::Any,
            "checkout grammar",
        )?;
        Ok(())
    }
}

fn fetch_grammar(grammar: GrammarConfiguration, runtime_directory: &Path) -> Result<FetchStatus> {
    let GrammarSource::Git {
        remote, revision, ..
    } = grammar.source
    else {
        unreachable!("local grammars are filtered before fetching");
    };

    let repo = VendoredGrammar::new(&grammar.grammar_id, runtime_directory);

    let (object_format, revision) = extract_object_format_from_revision(&revision);

    if repo.revision().is_some_and(|rev| rev == revision) {
        return Ok(FetchStatus::GitUpToDate);
    }

    // Fetch the grammar if the revision doesn't match.
    repo.fetch(&remote, revision, object_format)?;

    Ok(FetchStatus::GitUpdated {
        revision: revision.to_string(),
    })
}

enum BuildStatus {
    AlreadyBuilt,
    Built,
}

fn build_grammar(
    grammar: GrammarConfiguration,
    runtime_directory: &Path,
    target: Option<&str>,
) -> Result<BuildStatus> {
    let grammar_dir = if let GrammarSource::Local { path } = &grammar.source {
        PathBuf::from(&path)
    } else {
        runtime_directory
            .join("grammars")
            .join("sources")
            .join(&grammar.grammar_id)
    };

    let grammar_dir_entries = grammar_dir.read_dir().with_context(|| {
        format!(
            "Failed to read directory {:?}. Did you use 'ms --grammar fetch'?",
            grammar_dir
        )
    })?;

    if grammar_dir_entries.count() == 0 {
        return Err(anyhow!(
            "Directory {:?} is empty. Did you use 'ms --grammar fetch'?",
            grammar_dir
        ));
    };

    let path = match &grammar.source {
        GrammarSource::Git {
            subpath: Some(subpath),
            ..
        } => grammar_dir.join(subpath),
        _ => grammar_dir,
    }
    .join("src");

    build_tree_sitter_library(&path, grammar, runtime_directory, target)
}

fn build_tree_sitter_library(
    src_path: &Path,
    grammar: GrammarConfiguration,
    runtime_directory: &Path,
    target: Option<&str>,
) -> Result<BuildStatus> {
    let header_path = src_path;
    let parser_path = src_path.join("parser.c");
    let mut scanner_path = src_path.join("scanner.c");

    let scanner_path = if scanner_path.exists() {
        Some(scanner_path)
    } else {
        scanner_path.set_extension("cc");
        if scanner_path.exists() {
            Some(scanner_path)
        } else {
            None
        }
    };
    let parser_lib_path = runtime_directory.join("grammars");
    fs::create_dir_all(&parser_lib_path).context("Failed to create grammar output directory")?;
    let mut library_path = parser_lib_path.join(&grammar.grammar_id);
    library_path.set_extension(DYLIB_EXTENSION);

    let recompile = needs_recompile(&library_path, &parser_path, scanner_path.as_ref())
        .context("Failed to compare source and binary timestamps")?;

    if !recompile {
        return Ok(BuildStatus::AlreadyBuilt);
    }

    let mut config = cc::Build::new();
    config
        .cpp(true)
        .opt_level(3)
        .cargo_metadata(false)
        .host(BUILD_TARGET)
        .target(target.unwrap_or(BUILD_TARGET));
    let compiler = config.get_compiler();
    let mut command = compiler_command(&compiler, src_path);
    // used to delay dropping the temporary object file until after the compilation is complete
    let _path_guard;

    if compiler.is_like_msvc() {
        command
            .args(["/nologo", "/LD", "/I"])
            .arg(header_path)
            .arg("/utf-8")
            .arg("/std:c11");
        if let Some(scanner_path) = scanner_path.as_ref() {
            if scanner_path.extension() == Some("c".as_ref()) {
                command.arg(scanner_path);
            } else {
                let mut cpp_command = compiler_command(&compiler, src_path);
                let object_file =
                    library_path.with_file_name(format!("{}_scanner.obj", grammar.grammar_id));
                cpp_command
                    .args(["/nologo", "/LD", "/I"])
                    .arg(header_path)
                    .arg("/utf-8")
                    .arg("/std:c++14")
                    .arg(format!("/Fo{}", object_file.display()))
                    .arg("/c")
                    .arg(scanner_path);
                run_compiler(&mut cpp_command, "Failed to execute C++ compiler")?;
                command.arg(&object_file);
                _path_guard = TempPath::try_from_path(object_file).unwrap();
            }
        }

        command
            .arg(parser_path)
            .arg("/link")
            .arg(format!("/out:{}", library_path.to_str().unwrap()));
    } else {
        #[cfg(not(windows))]
        command.arg("-fPIC");

        command
            .arg("-shared")
            .arg("-fno-exceptions")
            .arg("-I")
            .arg(header_path)
            .arg("-o")
            .arg(&library_path);

        if let Some(scanner_path) = scanner_path.as_ref() {
            if scanner_path.extension() == Some("c".as_ref()) {
                command.arg("-xc").arg("-std=c11").arg(scanner_path);
            } else {
                let mut cpp_command = compiler_command(&compiler, src_path);
                let object_file =
                    library_path.with_file_name(format!("{}_scanner.o", grammar.grammar_id));

                #[cfg(not(windows))]
                cpp_command.arg("-fPIC");

                cpp_command
                    .arg("-fno-exceptions")
                    .arg("-I")
                    .arg(header_path)
                    .arg("-o")
                    .arg(&object_file)
                    .arg("-std=c++14")
                    .arg("-c")
                    .arg(scanner_path);
                run_compiler(&mut cpp_command, "Failed to execute C++ compiler")?;
                command.arg(&object_file);
                _path_guard = TempPath::try_from_path(object_file).unwrap();
            }
        }
        command.arg("-xc").arg("-std=c11").arg(parser_path);
        if cfg!(all(
            unix,
            not(any(target_os = "macos", target_os = "illumos"))
        )) {
            command.arg("-Wl,-z,relro,-z,now");
        }
    }

    run_compiler(&mut command, "Failed to execute C/C++ compiler")?;

    Ok(BuildStatus::Built)
}

fn compiler_command(compiler: &cc::Tool, src_path: &Path) -> Command {
    let mut command = Command::new(compiler.path());
    command.current_dir(src_path).args(compiler.args());
    for (key, value) in compiler.get_envs() {
        command.env(key, value);
    }
    command
}

fn run_compiler(command: &mut Command, context: &'static str) -> Result<()> {
    let output = command.output().context(context)?;
    if !output.status.success() {
        bail!(
            "Parser compilation failed.\nStdout: {}\nStderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn needs_recompile(
    lib_path: &Path,
    parser_c_path: &Path,
    scanner_path: Option<&PathBuf>,
) -> Result<bool> {
    if !lib_path.exists() {
        return Ok(true);
    }
    let lib_mtime = mtime(lib_path)?;
    if mtime(parser_c_path)? > lib_mtime {
        return Ok(true);
    }
    if let Some(scanner_path) = scanner_path
        && mtime(scanner_path)? > lib_mtime
    {
        return Ok(true);
    }
    Ok(false)
}

fn mtime(path: &Path) -> Result<SystemTime> {
    Ok(fs::metadata(path)?.modified()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git<I, S>(repository_dir: &Path, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository_dir)
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_owned())
        } else {
            Err(anyhow!(
                "Git command failed.\nStdout: {}\nStderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ))
        }
    }

    #[test]
    fn fetched_tag_checks_out_the_requested_revision() {
        let source = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        git(source.path(), ["init"]).unwrap();
        fs::write(source.path().join("parser.c"), "first parser\n").unwrap();
        git(source.path(), ["add", "parser.c"]).unwrap();
        git(
            source.path(),
            [
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "first",
            ],
        )
        .unwrap();
        git(source.path(), ["tag", "fixture-v1"]).unwrap();
        let expected = git(source.path(), ["rev-parse", "HEAD"]).unwrap();
        git(
            source.path(),
            [
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "second",
            ],
        )
        .unwrap();
        let grammar = VendoredGrammar::new("fixture", runtime.path());
        grammar
            .fetch(
                source.path().to_str().unwrap(),
                "fixture-v1",
                GitObjectFormat::Sha1,
            )
            .unwrap();
        assert_eq!(grammar.revision().unwrap(), expected);
        assert_eq!(
            fs::read_to_string(grammar.dir.join("parser.c")).unwrap(),
            "first parser\n"
        );
        // Full object IDs, including SHA-256, are used by pinned grammar definitions.
        let relative = Path::new("..")
            .join("..")
            .join("..")
            .join("..")
            .join(source.path().file_name().unwrap());
        grammar
            .fetch(relative.to_str().unwrap(), &expected, GitObjectFormat::Sha1)
            .unwrap();
        assert_eq!(grammar.revision().unwrap(), expected);
    }

    #[test]
    fn fetches_sha256_pinned_revision() {
        let source = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        git(source.path(), ["init", "--object-format=sha256"]).unwrap();
        fs::write(source.path().join("parser.c"), "parser\n").unwrap();
        git(source.path(), ["add", "parser.c"]).unwrap();
        git(
            source.path(),
            [
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "parser",
            ],
        )
        .unwrap();
        let expected = git(source.path(), ["rev-parse", "HEAD"]).unwrap();
        let grammar = VendoredGrammar::new("fixture", runtime.path());
        grammar
            .fetch(
                source.path().to_str().unwrap(),
                &expected,
                GitObjectFormat::Sha256,
            )
            .unwrap();
        assert_eq!(grammar.revision().unwrap(), expected);
        assert_eq!(
            fs::read_to_string(grammar.dir.join("parser.c")).unwrap(),
            "parser\n"
        );
    }

    #[test]
    fn local_grammar_build_uses_supplied_runtime_and_reuses_existing_library() {
        for scanner in [None, Some("scanner.c"), Some("scanner.cc")] {
            let source = tempfile::tempdir().unwrap();
            let runtime = tempfile::tempdir().unwrap();
            let src = source.path().join("src");
            fs::create_dir(&src).unwrap();
            fs::write(
                src.join("parser.c"),
                "int tree_sitter_fixture(void) { return 0; }\n",
            )
            .unwrap();
            if let Some(scanner) = scanner {
                fs::write(
                    src.join(scanner),
                    "int scanner_fixture(void) { return 0; }\n",
                )
                .unwrap();
            }
            let grammar = || GrammarConfiguration {
                grammar_id: "fixture".into(),
                source: GrammarSource::Local {
                    path: source.path().to_str().unwrap().into(),
                },
            };

            assert!(matches!(
                build_grammar(grammar(), runtime.path(), None).unwrap(),
                BuildStatus::Built
            ));
            let library = runtime
                .path()
                .join("grammars")
                .join("fixture")
                .with_extension(DYLIB_EXTENSION);
            assert!(library.is_file());
            let modified = mtime(&library).unwrap();
            assert!(matches!(
                build_grammar(grammar(), runtime.path(), None).unwrap(),
                BuildStatus::AlreadyBuilt
            ));
            assert_eq!(mtime(&library).unwrap(), modified);
            build_grammars(vec![grammar()], runtime.path(), None, true).unwrap();
            for extension in ["o", "obj"] {
                assert!(!runtime
                    .path()
                    .join(format!("grammars/fixture_scanner.{extension}"))
                    .exists());
            }
        }
    }
}
