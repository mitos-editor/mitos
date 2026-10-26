use std::process::{Command, Output};

fn assert_information_output(args: &[&str], output: &Output) {
    assert!(output.status.success(), "{args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    if args.contains(&"--help") || args.contains(&"-h") {
        assert!(stdout.contains("USAGE:"), "{args:?}: {stdout}");
        assert!(stdout.contains("--log <file>"), "{args:?}: {stdout}");
    } else {
        assert_eq!(stdout, format!("mitos {}\n", loader::VERSION_AND_GIT_HASH));
    }
}

const INFORMATION_ARGS: &[&[&str]] = &[
    &["--version"],
    &["-V"],
    &["--help"],
    &["-h"],
    &["--version", "--help"],
];

#[test]
fn information_flags_do_not_create_user_directories() {
    for args in INFORMATION_ARGS {
        let root = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_ms"))
            .args(*args)
            .env_clear()
            .env("HOME", root.path().join("home"))
            .env("USERPROFILE", root.path().join("home"))
            .env("APPDATA", root.path().join("config"))
            .env("LOCALAPPDATA", root.path().join("cache"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .current_dir(root.path())
            .output()
            .unwrap();
        assert_information_output(args, &output);
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "{args:?}"
        );
    }
}

#[test]
fn information_flags_work_without_home_environment_variables() {
    for args in INFORMATION_ARGS {
        let root = tempfile::tempdir().unwrap();
        // The OS may still supply a home directory. Explicit paths keep the test
        // from creating files there if startup regresses.
        let output = Command::new(env!("CARGO_BIN_EXE_ms"))
            .args(*args)
            .arg("--config")
            .arg(root.path().join("config/config.toml"))
            .arg("--log")
            .arg(root.path().join("cache/mitos.log"))
            .env_clear()
            .current_dir(root.path())
            .output()
            .unwrap();
        assert_information_output(args, &output);
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "{args:?}"
        );
    }
}
