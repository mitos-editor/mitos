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
        atomic::{AtomicUsize, Ordering},
        mpsc::channel,
        Arc,
    },
    time::SystemTime,
};
use tempfile::TempPath;

const BUILD_TARGET: &str = env!("BUILD_TARGET");
const REMOTE_NAME: &str = "origin";

fn ensure_git_is_available() -> Result<()> {
    stdx::env::which("git")?;
    Ok(())
}

pub fn fetch_grammars(
    grammars: Vec<GrammarConfiguration>,
    runtime_directory: &Path,
    strict: bool,
) -> Result<()> {
    ensure_git_is_available()?;

    // We do not need to fetch local grammars.
    let mut grammars = grammars;
    let runtime_directory = runtime_directory.to_owned();
    grammars.retain(|grammar| !matches!(grammar.source, GrammarSource::Local { .. }));

    let total = grammars.len();
    let counter = Arc::new(AtomicUsize::new(0));

    println!("Fetching {} grammars", total);
    let counter = Arc::clone(&counter);

    let results = run_parallel(grammars, move |grammar| {
        let current = counter.fetch_add(1, Ordering::Relaxed) + 1;

        println!(
            "Fetching grammars ({}/{}): {}",
            current, total, grammar.grammar_id
        );
        fetch_grammar(grammar, &runtime_directory)
    });

    let mut errors = Vec::new();
    let mut git_updated = Vec::new();
    let mut git_up_to_date = 0;
    let mut non_git = Vec::new();

    for (grammar_id, res) in results {
        match res {
            Ok(FetchStatus::GitUpToDate) => git_up_to_date += 1,
            Ok(FetchStatus::GitUpdated { revision }) => git_updated.push((grammar_id, revision)),
            Ok(FetchStatus::NonGit) => non_git.push(grammar_id),
            Err(e) => errors.push((grammar_id, e)),
        }
    }

    non_git.sort_unstable();
    git_updated.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    if git_up_to_date != 0 {
        println!("{} up to date git grammars", git_up_to_date);
    }

    if !non_git.is_empty() {
        println!("{} non git grammars", non_git.len());
        println!("\t{:?}", non_git);
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

    if !errors.is_empty() {
        let len = errors.len();
        for (i, (grammar, error)) in errors.into_iter().enumerate() {
            println!("Failure {}/{len}: {grammar} {error}", i + 1);
        }
        if strict {
            bail!("{len} grammars failed to fetch");
        }
    }

    Ok(())
}

pub fn build_grammars(
    grammars: Vec<GrammarConfiguration>,
    runtime_directory: &Path,
    target: Option<String>,
    strict: bool,
) -> Result<()> {
    ensure_git_is_available()?;

    let runtime_directory = runtime_directory.to_owned();

    let total = grammars.len();
    let counter = Arc::new(AtomicUsize::new(0));

    println!("Building {} grammars", grammars.len());

    let counter = Arc::clone(&counter);
    let results = run_parallel(grammars, move |grammar| {
        let current = counter.fetch_add(1, Ordering::Relaxed) + 1;

        println!(
            "Building grammars ({}/{}): {}",
            current, total, grammar.grammar_id
        );
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

    if !errors.is_empty() {
        let len = errors.len();
        for (i, (grammar_id, error)) in errors.into_iter().enumerate() {
            println!("Failure {}/{len}: {grammar_id} {error}", i + 1);
        }
        if strict {
            bail!("{len} grammars failed to build");
        }
    }

    Ok(())
}

fn run_parallel<F, Res>(grammars: Vec<GrammarConfiguration>, job: F) -> Vec<(String, Result<Res>)>
where
    F: Fn(GrammarConfiguration) -> Result<Res> + Send + 'static + Clone,
    Res: Send + 'static,
{
    let pool = threadpool::Builder::new().build();
    let (tx, rx) = channel();

    for grammar in grammars {
        let tx = tx.clone();
        let job = job.clone();

        pool.execute(move || {
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
    NonGit,
}

#[derive(Copy, Clone)]
enum GitObjectFormat {
    Sha1,
    Sha256,
}

impl GitObjectFormat {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
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

    /// Gets the current revision of the repo.
    fn revision(&self) -> Option<String> {
        git(&self.dir, ["rev-parse", "HEAD"]).ok()
    }

    /// Fetches grammar at the given revision.
    ///
    /// To ensure clean state, existing grammar directory is removed and re-inited
    /// before fetch operation.
    fn fetch(&self, remote: &str, rev: &str, object_format: GitObjectFormat) -> Result<()> {
        self.reinit(remote, object_format)?;

        git(&self.dir, ["fetch", "--depth", "1", REMOTE_NAME, rev])?;
        git(&self.dir, ["checkout", rev])?;

        Ok(())
    }

    /// Initializes the grammar directory.
    ///
    /// Creates directory and sets it up as a git repo, with remote set correctly.
    fn init(&self, remote: &str, object_format: GitObjectFormat) -> Result<()> {
        // Create the grammar directory if needed.
        fs::create_dir_all(&self.dir)
            .context(format!("Could not create grammar directory {:?}", self.dir))?;

        // Ensure directory is git initialized.
        if !self.dir.join(".git").exists() {
            git(
                &self.dir,
                ["init", "--object-format", object_format.as_str()],
            )?;
        }

        // Ensure the remote matches the configured remote, setting if needed.
        if self.remote().as_deref() != Some(remote) {
            self.set_remote(remote)?;
        }

        Ok(())
    }

    /// Removes the grammar directory before initializing again.
    fn reinit(&self, remote: &str, object_format: GitObjectFormat) -> Result<()> {
        fs::remove_dir_all(&self.dir)?;
        self.init(remote, object_format)?;
        Ok(())
    }

    /// Gets remote URL of grammar repo.
    fn remote(&self) -> Option<String> {
        git(&self.dir, ["remote", "get-url", REMOTE_NAME]).ok()
    }

    /// Sets remote URL of grammar repo.
    fn set_remote(&self, remote: &str) -> Result<()> {
        git(&self.dir, ["remote", "set-url", REMOTE_NAME, remote])
            .or_else(|_| git(&self.dir, ["remote", "add", REMOTE_NAME, remote]))?;
        Ok(())
    }
}

fn fetch_grammar(grammar: GrammarConfiguration, runtime_directory: &Path) -> Result<FetchStatus> {
    let GrammarSource::Git {
        remote, revision, ..
    } = grammar.source
    else {
        return Ok(FetchStatus::NonGit);
    };

    let repo = VendoredGrammar::new(&grammar.grammar_id, runtime_directory);

    let (object_format, revision) = extract_object_format_from_revision(&revision);

    // WARN: Must init before other operations are done.
    repo.init(&remote, object_format)?;

    if repo.revision().is_some_and(|rev| rev == revision) {
        return Ok(FetchStatus::GitUpToDate);
    }

    // Fetch the grammar if the revision doesn't match.
    repo.fetch(&remote, revision, object_format)?;

    Ok(FetchStatus::GitUpdated {
        revision: revision.to_string(),
    })
}

// A wrapper around 'git' commands which returns stdout in success and a
// helpful error message showing the command, stdout, and stderr in error.
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
        // TODO: figure out how to display the git command using `args`
        Err(anyhow!(
            "Git command failed.\nStdout: {}\nStderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ))
    }
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

    // if we are running inside a buildscript emit cargo metadata
    // to detect if we are running from a buildscript check some env variables
    // that cargo only sets for build scripts
    if std::env::var("OUT_DIR").is_ok() && std::env::var("CARGO").is_ok() {
        if let Some(scanner_path) = scanner_path.as_ref().and_then(|path| path.to_str()) {
            println!("cargo:rerun-if-changed={scanner_path}");
        }
        if let Some(parser_path) = parser_path.to_str() {
            println!("cargo:rerun-if-changed={parser_path}");
        }
    }

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
    let mut command = Command::new(compiler.path());
    command.current_dir(src_path);
    for (key, value) in compiler.get_envs() {
        command.env(key, value);
    }

    command.args(compiler.args());
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
                let mut cpp_command = Command::new(compiler.path());
                cpp_command.current_dir(src_path);
                for (key, value) in compiler.get_envs() {
                    cpp_command.env(key, value);
                }
                cpp_command.args(compiler.args());
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
                let output = cpp_command
                    .output()
                    .context("Failed to execute C++ compiler")?;

                if !output.status.success() {
                    return Err(anyhow!(
                        "Parser compilation failed.\nStdout: {}\nStderr: {}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
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
                let mut cpp_command = Command::new(compiler.path());
                cpp_command.current_dir(src_path);
                for (key, value) in compiler.get_envs() {
                    cpp_command.env(key, value);
                }
                cpp_command.args(compiler.args());
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
                let output = cpp_command
                    .output()
                    .context("Failed to execute C++ compiler")?;
                if !output.status.success() {
                    return Err(anyhow!(
                        "Parser compilation failed.\nStdout: {}\nStderr: {}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }

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

    let output = command
        .output()
        .context("Failed to execute C/C++ compiler")?;
    if !output.status.success() {
        return Err(anyhow!(
            "Parser compilation failed.\nStdout: {}\nStderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(BuildStatus::Built)
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

    #[test]
    fn local_grammar_build_uses_supplied_runtime_and_reuses_existing_library() {
        let source = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let src = source.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(
            src.join("parser.c"),
            "int tree_sitter_fixture(void) { return 0; }\n",
        )
        .unwrap();
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
    }
}
