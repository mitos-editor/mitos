//! Run path-discovery tests in a child process with its own config and trust store.
use std::{path::PathBuf, process::Command};

/// Returns the workspace in the child; the parent verifies the child and returns None.
/// Pass the full libtest name so exact filtering cannot accidentally run another suite.
pub fn workspace(test_name: &str) -> anyhow::Result<Option<PathBuf>> {
    const ROOT: &str = "MITOS_TEST_ISOLATED_ROOT";
    const TEST: &str = "MITOS_TEST_ISOLATED_NAME";
    if std::env::var(TEST).as_deref() == Ok(test_name) {
        let root = PathBuf::from(std::env::var_os(ROOT).expect("isolated child root"));
        assert!(loader::data_dir().starts_with(&root));
        assert!(loader::config_dir().starts_with(&root));
        loader::initialize_config_file(Some(root.join("config/mitos/config.toml")));
        return Ok(Some(root));
    }
    let dir = tempfile::tempdir()?;
    // Match document paths on Windows and resolve temporary-directory symlinks.
    let root = stdx::path::normalize(dir.path().canonicalize()?);
    for path in ["workspace/.mitos", "config/mitos", "data", "cache"] {
        std::fs::create_dir_all(root.join(path))?;
    }
    // Keep an explicit runtime selected by the test runner (including in worktrees).
    let runtime = std::env::var_os("MITOS_RUNTIME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime"));
    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .current_dir(root.join("workspace"))
        .env("MITOS_RUNTIME", runtime.canonicalize()?)
        .env(ROOT, &root)
        .env(TEST, test_name)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_STATE_HOME", root.join("data"))
        .env("APPDATA", root.join("config"))
        .env("LOCALAPPDATA", root.join("cache"))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "isolated test {test_name} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(None)
}
