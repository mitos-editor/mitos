use super::*;
use std::io::Write;

const FIXTURE: &str = "MITOS_COMMAND_PIPE_FIXTURE";

#[test]
fn subprocess_fixture() {
    let Ok(mode) = std::env::var(FIXTURE) else {
        return;
    };
    if mode == "large" {
        let bytes = [b'x'; 8192];
        for _ in 0..(OUTPUT_LIMIT / bytes.len() + 2) {
            std::io::stdout().write_all(&bytes).unwrap();
            std::io::stderr().write_all(&bytes).unwrap();
        }
    } else {
        print!("private stdout fixture");
        eprintln!("private stderr fixture\nsecond\tline");
        std::process::exit(7);
    }
}

fn fixture(mode: &str) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            "command::tests::subprocess_fixture",
            "--nocapture",
        ])
        .env(FIXTURE, mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

#[tokio::test]
async fn async_output_drains_both_pipes_and_keeps_private_errors_private() -> Result<()> {
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        run(fixture("large")?.into()),
    )
    .await?
    .unwrap_err();
    assert!(error.to_string().contains("1 MiB"));
    let error = run_private(fixture("fail")?.into(), "Fixture operation")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Fixture operation failed"));
    assert!(!error.contains("private stdout fixture"));
    assert!(!error.contains("private stderr fixture"));
    let error = run(fixture("fail")?.into()).await.unwrap_err().to_string();
    assert!(error.contains("private stderr fixture second line"));
    assert!(!error.contains('\n'));
    Ok(())
}

#[test]
fn synchronous_output_drains_both_pipes_with_the_same_limits_and_error_policy() -> Result<()> {
    let error = run_sync(fixture("large")?, Output::Display).unwrap_err();
    assert!(error.to_string().contains("1 MiB"));
    let error = run_sync(fixture("fail")?, Output::Private("Fixture operation"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Fixture operation failed"));
    assert!(!error.contains("private stdout fixture"));
    assert!(!error.contains("private stderr fixture"));
    Ok(())
}

#[tokio::test]
async fn repository_commands_pin_paths_and_remove_inherited_configuration() -> Result<()> {
    const ROOT: &str = "MITOS_COMMAND_REPOSITORY_FIXTURE";
    if let Some(root) = std::env::var_os(ROOT) {
        let root = std::path::PathBuf::from(root);
        for access in [Access::ReadOnly, Access::Mutation] {
            let mut command = repository(&root, &root.join(".git"), access);
            let lazy_fetch = command
                .get_envs()
                .find(|(name, _)| *name == "GIT_NO_LAZY_FETCH");
            assert_eq!(
                lazy_fetch.and_then(|(_, value)| value),
                matches!(access, Access::ReadOnly).then_some(std::ffi::OsStr::new("1"))
            );
            command.args(["config", "--get", "mitos.commandRunner"]);
            assert_eq!(run_sync(command, Output::Display)?, b"selected\n");
        }
        let mut command = repository(&root, &root.join(".git"), Access::ReadOnly);
        command.args(["config", "--get", "mitos.commandRunner"]);
        assert_eq!(run(command.into()).await?, b"selected\n");
        return Ok(());
    }
    let selected = crate::git::test::empty_git_repo();
    let other = crate::git::test::empty_git_repo();
    crate::git::test::exec_git_cmd(
        &["config", "mitos.commandRunner", "selected"],
        selected.path(),
    );
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "command::tests::repository_commands_pin_paths_and_remove_inherited_configuration",
            "--nocapture",
        ])
        .env(ROOT, selected.path())
        .env("GIT_DIR", other.path().join(".git"))
        .env("GIT_WORK_TREE", other.path())
        .env("GIT_CONFIG", other.path().join(".git/config"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "mitos.commandRunner")
        .env("GIT_CONFIG_VALUE_0", "inherited")
        .env("GIT_CONFIG_PARAMETERS", "'mitos.commandRunner=inherited'")
        .env("GIT_OBJECT_DIRECTORY", other.path().join("missing objects"))
        .output()?;
    ensure!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn structured_stdout_streams_above_display_limit_and_drains_after_parse_errors() -> Result<()> {
    let bytes = read_sync(fixture("large")?, |stdout| {
        Ok(std::io::copy(stdout, &mut std::io::sink())?)
    })?;
    assert!(bytes > OUTPUT_LIMIT as u64);
    let error = read_sync::<()>(fixture("large")?, |_| {
        anyhow::bail!("Invalid structured fixture")
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "Invalid structured fixture");
    Ok(())
}
