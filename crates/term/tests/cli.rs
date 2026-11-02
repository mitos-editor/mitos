use std::process::{Command, Output};

fn isolated_command(root: &std::path::Path, with_home: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ms"));
    command.env_clear().current_dir(root);
    if with_home {
        for (key, directory) in [
            ("HOME", "home"),
            ("USERPROFILE", "home"),
            ("APPDATA", "config"),
            ("LOCALAPPDATA", "cache"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            command.env(key, root.join(directory));
        }
    } else {
        // Guard the OS-provided home directory if startup regresses.
        command
            .arg("--config")
            .arg(root.join("config/config.toml"))
            .arg("--log")
            .arg(root.join("cache/mitos.log"));
    }
    command
}

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
fn language_health_shows_configured_tool_arguments() {
    let executable = env!("CARGO_BIN_EXE_ms");
    for command in [executable, "missing-mitos-health-tool"] {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join("config/mitos");
        std::fs::create_dir_all(&config_dir).unwrap();
        let command = toml::Value::String(command.to_owned());
        std::fs::write(
            config_dir.join("languages.toml"),
            format!(
                r#"
[language-server.health-test]
command = {command}
args = ["lsp", "--stdio"]
[[language]]
name = "health-test"
scope = "source.health-test"
file-types = []
language-servers = ["health-test"]
formatter = {{ command = {command}, args = ["format", "%{{buffer_name}}"] }}
[language.debugger]
name = "health-test"
transport = "stdio"
command = {command}
args = ["debug", "--stdio"]
templates = []
"#
            ),
        )
        .unwrap();
        let output = isolated_command(root.path(), true)
            .args(["--health", "health-test"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = std::str::from_utf8(&output.stdout).unwrap();
        for arguments in [
            r#"Arguments: ["lsp", "--stdio"]"#,
            r#"Arguments: ["format", "%{buffer_name}"]"#,
            r#"Arguments: ["debug", "--stdio"]"#,
        ] {
            assert!(stdout.contains(arguments), "{stdout}");
        }
    }
}

#[test]
fn information_flags_do_not_create_user_directories() {
    for args in INFORMATION_ARGS {
        let root = tempfile::tempdir().unwrap();
        let output = isolated_command(root.path(), true)
            .args(*args)
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
        let output = isolated_command(root.path(), false)
            .args(*args)
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
