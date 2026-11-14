use std::{fs, process::Command};

#[test]
fn language_health_respects_workspace_trust() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let project = root.path().join("workspace");
    let config = root.path().join("config/mitos");
    fs::create_dir_all(project.join(".mitos"))?;
    fs::create_dir_all(&config)?;
    fs::write(
        project.join(".mitos/languages.toml"),
        "[[language]]\nname = 'health-trust-test'\nscope = 'source.test'\nfile-types = ['health-test']\n",
    )?;
    for (level, visible) in [("none", false), ("insecure", true)] {
        fs::write(
            config.join("config.toml"),
            format!("[editor.workspace-trust]\nlevel = '{level}'\n"),
        )?;
        for category in ["health-trust-test", "all-languages"] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_ms"));
            command
                .args(["--config", config.join("config.toml").to_str().unwrap()])
                .args(["--health", category])
                .current_dir(&project)
                .env_remove("MITOS_CONFIG_DIR")
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("XDG_DATA_HOME", root.path().join("data"))
                .env("XDG_CACHE_HOME", root.path().join("cache"))
                .env("XDG_STATE_HOME", root.path().join("state"))
                .env("APPDATA", root.path().join("config"))
                .env("LOCALAPPDATA", root.path().join("cache"));
            let output = command.output()?;
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("was skipped because the workspace is not trusted"),
                !visible
            );
            let stdout = String::from_utf8(output.stdout)?;
            assert!(!stdout.contains("was skipped because the workspace is not trusted"));
            if !visible {
                let (reader, writer) = std::io::pipe()?;
                drop(reader);
                let output = command.stdout(writer).output()?;
                assert!(output.status.success(), "closed pipe: {output:?}");
                assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
            }
            if category == "health-trust-test" {
                assert_eq!(
                    stdout.contains("Language 'health-trust-test' not found"),
                    !visible,
                    "{stdout}"
                );
            } else {
                assert!(
                    stdout.lines().next().unwrap().starts_with("Lang"),
                    "{stdout}"
                );
            }
        }
    }
    Ok(())
}
