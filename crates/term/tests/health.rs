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
                .env("TOKIO_WORKER_THREADS", "2")
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

#[cfg(feature = "integration")]
mod capabilities {
    use super::*;
    use std::{
        path::Path,
        time::{Duration, Instant},
    };

    struct Setup {
        root: tempfile::TempDir,
        project: std::path::PathBuf,
        config: std::path::PathBuf,
    }

    impl Setup {
        fn new(languages: &str) -> anyhow::Result<Self> {
            let root = tempfile::tempdir()?;
            let project = root.path().join("workspace");
            let config = root.path().join("config/mitos");
            fs::create_dir_all(&project)?;
            fs::create_dir_all(&config)?;
            fs::write(
                config.join("config.toml"),
                "[editor.workspace-trust]\nlevel = 'none'\n",
            )?;
            fs::write(config.join("languages.toml"), languages)?;
            Ok(Self {
                root,
                project,
                config,
            })
        }

        fn run(&self, category: &str) -> anyhow::Result<String> {
            let output = Command::new(env!("CARGO_BIN_EXE_ms"))
                .args([
                    "--config",
                    self.config.join("config.toml").to_str().unwrap(),
                    "--health",
                    category,
                ])
                .current_dir(&self.project)
                .env_remove("MITOS_CONFIG_DIR")
                .env("TOKIO_WORKER_THREADS", "2")
                .env("XDG_CONFIG_HOME", self.root.path().join("config"))
                .env("XDG_DATA_HOME", self.root.path().join("data"))
                .env("XDG_CACHE_HOME", self.root.path().join("cache"))
                .env("XDG_STATE_HOME", self.root.path().join("state"))
                .env("APPDATA", self.root.path().join("config"))
                .env("LOCALAPPDATA", self.root.path().join("cache"))
                .output()?;
            assert!(output.status.success(), "{output:?}");
            let stdout = String::from_utf8(output.stdout)?;
            assert!(
                !stdout.contains('\u{1b}'),
                "redirected health output contains ANSI escapes"
            );
            Ok(stdout)
        }
    }

    fn quoted(path: impl AsRef<Path>) -> String {
        toml::Value::String(path.as_ref().to_string_lossy().into_owned()).to_string()
    }

    fn assert_probe_stopped(log: &Path) -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            let pid: i32 = fs::read_to_string(log.with_extension("pid"))?.parse()?;
            for _ in 0..50 {
                // SAFETY: signal zero checks existence without sending a signal.
                if unsafe { libc::kill(pid, 0) } == -1 {
                    assert_eq!(
                        std::io::Error::last_os_error().raw_os_error(),
                        Some(libc::ESRCH)
                    );
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("health probe process {pid} survived the command");
        }
        #[cfg(not(unix))]
        {
            let _ = log;
            Ok(())
        }
    }

    #[test]
    fn negotiates_lsp_capabilities_with_project_environment_and_configuration() -> anyhow::Result<()>
    {
        let log_dir = tempfile::tempdir()?;
        let log = log_dir.path().join("lsp.log");
        let setup = Setup::new(&format!(
            r#"
[language-server.health-fixture]
command = {command}
args = ["lsp", {log}]
environment = {{ MITOS_HEALTH_FIXTURE = "present" }}
config = {{ health = "configured" }}
[[language]]
name = "health-fixture"
scope = "source.health-fixture"
file-types = []
language-servers = [{{ name = "health-fixture", except-features = ["completion"] }}]
"#,
            command = quoted(env!("CARGO_BIN_EXE_mitos-test-health")),
            log = quoted(&log)
        ))?;
        let output = setup.run("health-fixture")?;
        assert!(output.contains("✓ Supported:\n      format"), "{output}");
        assert!(output.contains("hover"), "{output}");
        let unsupported = output
            .split("− Not advertised:")
            .nth(1)
            .unwrap()
            .split("○ Disabled")
            .next()
            .unwrap();
        assert!(unsupported.contains("rename-symbol"), "{output}");
        assert!(
            output.contains("○ Disabled by configuration:\n      completion"),
            "{output}"
        );
        assert!(
            output.contains("Push diagnostics: not advertised by LSP"),
            "{output}"
        );
        assert_probe_stopped(&log)?;
        let log = fs::read_to_string(log)?;
        assert!(log.contains("initialize\ninitialized\nshutdown"), "{log}");
        Ok(())
    }

    #[test]
    fn reports_stdio_and_tcp_dap_capabilities_without_launching_a_debuggee() -> anyhow::Result<()> {
        for transport in ["stdio", "tcp"] {
            let log_dir = tempfile::tempdir()?;
            let log = log_dir.path().join("dap.log");
            let mode = if transport == "stdio" { "dap" } else { "tcp" };
            let setup = Setup::new(&format!(
                r#"
[[language]]
name = "health-dap"
scope = "source.health-dap"
file-types = []
[language.debugger]
name = "health-dap"
transport = "{transport}"
command = {command}
args = ["{mode}", {log}]
port-arg = "--port={{}}"
templates = []
"#,
                command = quoted(env!("CARGO_BIN_EXE_mitos-test-health")),
                log = quoted(&log)
            ))?;
            let output = setup.run("health-dap")?;
            assert!(
                output.contains("✓ Supported:\n      conditional-breakpoints, log-points"),
                "{output}"
            );
            let unsupported = output.split("− Not advertised:").nth(1).unwrap();
            assert!(unsupported.contains("step-back"), "{output}");
            assert!(unsupported.contains("set-variable"), "{output}");
            assert_probe_stopped(&log)?;
            assert_eq!(
                fs::read_to_string(log)?,
                "initialize\nrunInTerminal\ndisconnect\n"
            );
        }
        Ok(())
    }

    #[test]
    fn timeout_and_early_exit_report_unknown_capabilities_and_continue() -> anyhow::Result<()> {
        let log_dir = tempfile::tempdir()?;
        let log = log_dir.path().join("startup.log");
        let exited = log_dir.path().join("exited.log");
        let skipped = log_dir.path().join("skipped.log");
        let setup = Setup::new(&format!(
            r#"
[language-server.health-hang]
command = {command}
args = ["hang", {log}]
[language-server.health-exit]
command = {command}
args = ["fail", {exited}]
[language-server.health-skipped]
command = {command}
args = ["fail", {skipped}]
required-root-patterns = ["__mitos_missing_health_project_marker__"]
[language-server.health-missing]
command = "/__mitos_missing_health_executable__"
[[language]]
name = "health-failure"
scope = "source.health-failure"
file-types = []
language-servers = [{{ alternatives = ["health-hang", "health-exit", "health-skipped", "health-missing", "health-undefined"] }}]
"#,
            command = quoted(env!("CARGO_BIN_EXE_mitos-test-health")),
            log = quoted(&log),
            exited = quoted(&exited),
            skipped = quoted(&skipped)
        ))?;
        let start = Instant::now();
        let output = setup.run("health-failure")?;
        assert!(start.elapsed() < Duration::from_secs(15));
        assert!(
            output.contains("Initialization timed out after 5s; capabilities unknown"),
            "{output}"
        );
        assert!(
            output.contains("Initialization failed; capabilities unknown"),
            "{output}"
        );
        assert!(
            output.contains("Capabilities unknown: definition missing"),
            "{output}"
        );
        assert!(output.contains("Tree-sitter"), "{output}");
        assert!(!output.contains("✓ Supported:"), "{output}");
        assert!(
            output.contains("Capabilities unknown: executable missing"),
            "{output}"
        );
        assert!(
            output.contains("Skipped: project does not match required-root-patterns"),
            "{output}"
        );
        assert!(!skipped.exists());
        assert_probe_stopped(&log)?;
        assert_probe_stopped(&exited)?;
        Ok(())
    }

    #[test]
    fn dap_timeout_stops_the_adapter_and_keeps_reporting_other_checks() -> anyhow::Result<()> {
        let log_dir = tempfile::tempdir()?;
        let log = log_dir.path().join("dap-timeout.log");
        let setup = Setup::new(&format!(
            r#"
[[language]]
name = "health-dap-timeout"
scope = "source.health-dap-timeout"
file-types = []
[language.debugger]
name = "health-dap"
transport = "stdio"
command = {command}
args = ["hang", {log}]
templates = []
"#,
            command = quoted(env!("CARGO_BIN_EXE_mitos-test-health")),
            log = quoted(&log)
        ))?;
        let start = Instant::now();
        let output = setup.run("health-dap-timeout")?;
        assert!(start.elapsed() < Duration::from_secs(15));
        assert!(
            output.contains("Initialization timed out after 5s; capabilities unknown"),
            "{output}"
        );
        assert!(output.contains("Tree-sitter"), "{output}");
        assert!(!output.contains("✓ Supported:"), "{output}");
        assert_probe_stopped(&log)?;
        Ok(())
    }

    #[test]
    fn overview_never_launches_servers_and_counts_missing_definitions() -> anyhow::Result<()> {
        let log_dir = tempfile::tempdir()?;
        let log = log_dir.path().join("must-not-exist.log");
        let setup = Setup::new(&format!(
            r#"
[language-server.health-installed]
command = {command}
args = ["fail", {log}]
[language-server.health-missing]
command = "/__mitos_missing_health_executable__"
[[language]]
name = "health-very-long-language-name"
scope = "source.health-overview"
file-types = []
language-servers = ["health-installed", "health-missing", "health-undefined"]
"#,
            command = quoted(env!("CARGO_BIN_EXE_mitos-test-health")),
            log = quoted(&log)
        ))?;
        let output = setup.run("all-languages")?;
        assert!(
            output.contains("health-very-long-language-name"),
            "{output}"
        );
        assert!(output.contains("1/3"), "{output}");
        assert!(!log.exists());
        Ok(())
    }
}
