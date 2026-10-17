use dap::requests::RunInTerminalArguments;
use serde_json::{json, Value};
use std::collections::HashMap;

#[tokio::test(flavor = "multi_thread")]
async fn debuggee_runs_with_requested_directory_environment_and_literal_arguments(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let working = dir.path().join("space # and ünicode");
    std::fs::create_dir(&working)?;
    let output = dir.path().join("result.json");
    let expected = vec!["space in argument", "'quotes' $literal & ;", "ünicode"];
    let mut args = vec![
        env!("CARGO_BIN_EXE_mitos-test-debuggee").to_owned(),
        output.to_string_lossy().into_owned(),
    ];
    args.extend(expected.iter().map(|value| value.to_string()));
    for cwd in [
        working.to_string_lossy().into_owned(),
        stdx::Url::from_file_path(&working).unwrap().to_string(),
    ] {
        let handoff = tempfile::tempdir()?;
        let request = RunInTerminalArguments {
            kind: Some("external".into()),
            title: None,
            cwd,
            args: args.clone(),
            env: Some(HashMap::from([
                ("MITOS_DEBUG_VALUE".into(), Some("requested".into())),
                ("MITOS_DEBUG_REMOVE".into(), None),
            ])),
        };
        std::fs::write(
            handoff.path().join("request.json"),
            serde_json::to_vec(&request)?,
        )?;
        let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_ms"))
            .arg("--internal-debug-terminal")
            .arg(handoff.path())
            .env("MITOS_DEBUG_REMOVE", "inherited")
            .status()
            .await?;
        assert!(status.success());
        let actual: Value = serde_json::from_slice(&std::fs::read(&output)?)?;
        let response: Value =
            serde_json::from_slice(&std::fs::read(handoff.path().join("response.json"))?)?;
        assert_eq!(actual["args"], json!(expected));
        assert_eq!(actual["env"]["MITOS_DEBUG_VALUE"], "requested");
        assert_eq!(actual["env"]["MITOS_DEBUG_REMOVE"], Value::Null);
        assert_eq!(
            stdx::path::canonicalize_existing(std::path::Path::new(
                actual["cwd"].as_str().unwrap()
            )),
            stdx::path::canonicalize_existing(&working)
        );
        assert_eq!(response["Process"], actual["pid"]);
        assert!(!handoff.path().join("request.json").exists());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_helper_reports_debuggee_start_failures() -> anyhow::Result<()> {
    let handoff = tempfile::tempdir()?;
    let request = RunInTerminalArguments {
        kind: None,
        title: None,
        cwd: String::new(),
        args: vec![handoff
            .path()
            .join("missing-program")
            .to_string_lossy()
            .into_owned()],
        env: None,
    };
    std::fs::write(
        handoff.path().join("request.json"),
        serde_json::to_vec(&request)?,
    )?;
    let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_ms"))
        .arg("--internal-debug-terminal")
        .arg(handoff.path())
        .output()
        .await?;
    assert!(!result.status.success());
    let response: Value =
        serde_json::from_slice(&std::fs::read(handoff.path().join("response.json"))?)?;
    assert!(response["Failed"]
        .as_str()
        .unwrap()
        .contains("Could not start debuggee"));
    assert!(!handoff.path().join("request.json").exists());
    Ok(())
}
