use std::path::Path;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn watch(path: impl AsRef<Path>) {
    let path = path.as_ref();
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    // Build metadata needs no user configuration, credentials, or Git executable.
    let options = gix::open::Options::isolated();
    let repo = gix::ThreadSafeRepository::discover_opts(
        env!("CARGO_MANIFEST_DIR"),
        Default::default(),
        gix::sec::trust::Mapping {
            full: options.clone(),
            reduced: options,
        },
    )
    .ok()
    .map(|repo| repo.to_thread_local());
    if let Some(repo) = &repo {
        watch(repo.git_dir().join("HEAD"));
        watch(repo.common_dir().join("packed-refs"));
        // A commit can create a loose ref for a previously packed branch.
        watch(repo.common_dir().join("refs/heads"));
        if let Ok(Some(head)) = repo.head_name() {
            watch(repo.common_dir().join(gix::path::from_bstr(head.as_bstr())));
        }
    }
    let revision = repo
        .as_ref()
        .and_then(|repo| repo.head_id().ok().map(|id| id.to_string()))
        .or_else(|| option_env!("MITOS_NIX_BUILD_REV").map(str::to_owned));
    let version = match &revision {
        Some(revision) => format!("{} ({})", VERSION, revision.get(..8).unwrap_or(revision)),
        None => VERSION.to_owned(),
    };
    println!("cargo:rustc-env=VERSION_AND_GIT_HASH={version}");
}
