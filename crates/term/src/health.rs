mod output;
mod probes;

pub use output::Options;
use output::{Output, Status};

use crate::config::{Config, ConfigLoadError};
use editor_core::unicode::width::UnicodeWidthStr;
use language::loader::{default_lang_config, parse_lang_config};
use language::support::LanguageSupport;
use loader::syntax::Resources;
use std::{collections::HashSet, io::Write};
use termina::{style::ColorSpec, Terminal as _};

fn write_status_cell(stdout: &mut Output<impl Write>, value: &str) -> std::io::Result<()> {
    if let Some((installed, configured)) = value.split_once('/') {
        let color = if installed == "0" {
            ColorSpec::BRIGHT_RED
        } else if installed == configured {
            ColorSpec::BRIGHT_GREEN
        } else {
            ColorSpec::BRIGHT_YELLOW
        };
        return write!(stdout, "{}", stdout.styled(value, color));
    }

    for (index, mark) in value.split(' ').enumerate() {
        if index != 0 {
            write!(stdout, " ")?;
        }
        let status = match mark {
            "✓" => Status::Ok,
            "✘" => Status::Error,
            _ => {
                write!(stdout, "{mark}")?;
                continue;
            }
        };
        write!(stdout, "{}", stdout.marker(status))?;
    }
    Ok(())
}

fn language_sources() -> std::io::Result<loader::config::LanguageSources> {
    let workspace = workspace::ResolvedWorkspace::current();
    let config = Config::load_for(&workspace).unwrap_or_default();
    let trust = workspace::trust::WorkspaceTrust::new(
        (&config.editor.workspace_trust).into(),
        loader::workspace_trust_dir(),
    );
    let sources = loader::config::ConfigSources::new(&workspace, &trust).languages();
    if let Some(diagnostic) = &sources.skipped {
        writeln!(std::io::stderr().lock(), "{diagnostic}")?;
    }
    Ok(sources)
}

/// Display general diagnostics.
fn general(stdout: &mut Output<impl Write>) -> std::io::Result<()> {
    let config_file = loader::config_file();
    let lang_file = loader::lang_config_file();
    let log_file = loader::log_file();
    let rt_dirs = loader::runtime_dirs();

    stdout.heading(&format!("Mitos {}", loader::VERSION_AND_GIT_HASH))?;
    writeln!(stdout)?;
    stdout.heading("Configuration")?;
    if config_file.exists() {
        stdout.row(
            Status::Info,
            "config",
            if stdout.options.summary {
                "custom file".into()
            } else {
                config_file.display().to_string()
            },
        )?;
    } else {
        stdout.row(Status::Info, "config", "default")?;
    }
    if lang_file.exists() {
        stdout.row(
            Status::Info,
            "languages",
            if stdout.options.summary {
                "custom file".into()
            } else {
                lang_file.display().to_string()
            },
        )?;
    } else {
        stdout.row(Status::Info, "languages", "default")?;
    }
    if !stdout.options.summary {
        stdout.row(Status::Info, "log", log_file.display())?;
    }
    writeln!(stdout)?;
    stdout.heading("Runtime directories (in search order)")?;
    if stdout.options.summary {
        stdout.row(
            Status::Info,
            "search paths",
            format_args!("{} directories", rt_dirs.len()),
        )?;
    } else {
        for directory in rt_dirs {
            stdout.row(Status::Info, "search path", directory.display())?;
        }
    }
    runtime_warnings(stdout, rt_dirs)
}

fn runtime_warnings(
    stdout: &mut Output<impl Write>,
    rt_dirs: &[std::path::PathBuf],
) -> std::io::Result<()> {
    for rt_dir in rt_dirs.iter() {
        if let Ok(path) = std::fs::read_link(rt_dir) {
            let msg = format!(
                "Runtime directory {} is symlinked to: {}",
                rt_dir.display(),
                path.display()
            );
            stdout.row(Status::Warning, "runtime", msg)?;
        }
        if !rt_dir.exists() {
            let msg = format!("Runtime directory does not exist: {}", rt_dir.display());
            stdout.row(Status::Warning, "runtime", msg)?;
        } else if rt_dir.read_dir().ok().map(|it| it.count()) == Some(0) {
            let msg = format!("Runtime directory is empty: {}", rt_dir.display());
            stdout.row(Status::Warning, "runtime", msg)?;
        }
    }
    let has_grammars = rt_dirs.iter().any(|directory| {
        directory
            .join("grammars")
            .read_dir()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| {
                let path = entry.path();
                path.extension()
                    .is_some_and(|extension| extension == loader::grammar::DYLIB_EXTENSION)
                    && path.is_file()
            })
    });
    if !has_grammars {
        stdout.row(Status::Warning, "grammars", "No compiled tree-sitter grammars found. Syntax highlighting requires compiled grammars.")?;
        stdout.detail(
            "For source builds, fetch and compile grammars before installing the runtime:",
        )?;
        stdout.remedy("https://mitos.computer/docs/building-from-source.html")?;
    }
    Ok(())
}

fn clipboard(stdout: &mut Output<impl Write>) -> std::io::Result<()> {
    stdout.heading("Clipboard")?;
    let config = match Config::load_default() {
        Ok(config) => config,
        Err(ConfigLoadError::Error(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            Config::default()
        }
        Err(err) => {
            stdout.row(Status::Error, "config", "Configuration file malformed")?;
            stdout.note(err)?;
            return Ok(());
        }
    };

    use ::clipboard::ClipboardBackend;
    match crate::clipboard::TerminalClipboard
        .name(&config.editor.clipboard.provider)
        .as_str()
    {
        "none" => {
            stdout.row(Status::Warning, "provider", "Not installed")?;
            stdout.detail("For troubleshooting system clipboard issues, refer to:")?;
            stdout.remedy("https://github.com/helix-editor/helix/wiki/Troubleshooting#copypaste-fromto-system-clipboard-not-working")?;
        }
        name => stdout.row(Status::Ok, "provider", name)?,
    }

    Ok(())
}

fn languages_all(stdout: &mut Output<impl Write>) -> std::io::Result<()> {
    languages(stdout, None, language_sources()?.value)
}

fn languages_selection(stdout: &mut Output<impl Write>) -> std::io::Result<()> {
    let mut value = language_sources()?.value;
    let selection = value
        .as_mut()
        .ok()
        .and_then(|value| loader::grammar::get_grammar_names(value).ok())
        .flatten();
    languages(stdout, selection, value)
}

fn languages(
    stdout: &mut Output<impl Write>,
    selection: Option<HashSet<String>>,
    value: Result<toml::Value, toml::de::Error>,
) -> std::io::Result<()> {
    let mut syn_loader_conf = match value
        .map_err(language::loader::LanguageLoaderError::DeserializeError)
        .and_then(parse_lang_config)
    {
        Ok(conf) => conf,
        Err(err) => {
            let stderr = std::io::stderr();
            let mut stderr = stderr.lock();

            writeln!(
                stderr,
                "{}: {}",
                stdout.styled("Error parsing user language config", ColorSpec::BRIGHT_RED),
                err
            )?;
            writeln!(
                stderr,
                "{}",
                stdout.styled("Using default language config", ColorSpec::BRIGHT_YELLOW)
            )?;
            default_lang_config()
        }
    };

    let resources = Resources::default();
    syn_loader_conf
        .language
        .sort_unstable_by(|a, b| a.language_id.cmp(&b.language_id));
    let mut rows = vec![vec![
        "Language".to_owned(),
        "LSP".to_owned(),
        "DAP".to_owned(),
        "Formatter".to_owned(),
        "H T I N R F".to_owned(),
    ]];
    for lang in &syn_loader_conf.language {
        if selection
            .as_ref()
            .is_some_and(|s| !s.contains(&lang.language_id))
        {
            continue;
        }
        let installed = lang
            .language_servers
            .iter()
            .filter(|server| {
                syn_loader_conf
                    .language_server
                    .get(&server.name)
                    .is_some_and(|definition| stdx::env::which(&definition.command).is_ok())
            })
            .count();
        let lsp = if lang.language_servers.is_empty() {
            "−".to_owned()
        } else {
            format!("{installed}/{}", lang.language_servers.len())
        };
        let binary = |command: Option<&str>| {
            match command {
                None => "−",
                Some(command) if stdx::env::which(command).is_ok() => "✓",
                Some(_) => "✘",
            }
            .to_owned()
        };
        let queries = LanguageSupport::inspect(&resources, &lang.language_id)
            .queries
            .into_iter()
            .map(|(_, source)| if source.is_readable() { "✓" } else { "✘" })
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(vec![
            lang.language_id.clone(),
            lsp,
            binary(lang.debugger.as_ref().map(|dap| dap.command.as_str())),
            binary(
                lang.formatter
                    .as_ref()
                    .map(|formatter| formatter.command.as_str()),
            ),
            queries,
        ]);
    }
    write_table(stdout, &rows, terminal_columns())?;
    writeln!(
        stdout,
        "\nLSP: installed/configured executables (including alternatives)."
    )?;
    writeln!(stdout, "✓ found  ✘ missing  − not configured")?;
    writeln!(
        stdout,
        "Queries: H highlight, T textobject, I indent, N tags, R rainbow, F folding."
    )?;
    writeln!(
        stdout,
        "Run ms --health <language> for tool paths and live LSP/DAP capabilities."
    )?;

    if selection.is_some() {
        writeln!(
            stdout,
            "\nThis list is filtered according to the 'use-grammars' option in languages.toml file.\n\
            To see the full list, use the '--health all' or '--health all-languages' option."
        )?;
    }

    Ok(())
}

/// Keep complete names, falling back to labelled rows on narrow terminals.
fn write_table(
    stdout: &mut Output<impl Write>,
    rows: &[Vec<String>],
    columns: usize,
) -> std::io::Result<()> {
    let Some(headings) = rows.first() else {
        return Ok(());
    };
    let widths: Vec<usize> = (0..headings.len())
        .map(|column| {
            rows.iter()
                .map(|row| row[column].width())
                .max()
                .unwrap_or(0)
        })
        .collect();
    if widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1) > columns {
        stdout.heading("Languages")?;
        for row in &rows[1..] {
            writeln!(stdout, "\n{}", row[0])?;
            for (heading, value) in headings[1..].iter().zip(&row[1..]) {
                write!(stdout, "  {heading}: ")?;
                write_status_cell(stdout, value)?;
                writeln!(stdout)?;
            }
        }
        return Ok(());
    }
    for (index, row) in rows.iter().enumerate() {
        for (column, (value, width)) in row.iter().zip(&widths).enumerate() {
            if column != 0 {
                write!(stdout, "  ")?;
            }
            if index == 0 {
                write!(stdout, "{}", stdout.bold(value))?;
            } else if column == 0 {
                write!(stdout, "{value}")?;
            } else {
                write_status_cell(stdout, value)?;
            }
            if column + 1 < row.len() {
                write!(stdout, "{}", " ".repeat(width - value.width()))?;
            }
        }
        writeln!(stdout)?;
        if index == 0 {
            writeln!(
                stdout,
                "{}",
                widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("  ")
            )?;
        }
    }
    Ok(())
}

/// Display diagnostics pertaining to a particular language (LSP,
/// highlight queries, etc).
async fn language(stdout: &mut Output<impl Write>, lang_str: String) -> std::io::Result<()> {
    let syn_loader_conf = match language_sources()?
        .value
        .map_err(language::loader::LanguageLoaderError::DeserializeError)
        .and_then(parse_lang_config)
    {
        Ok(conf) => conf,
        Err(err) => {
            let stderr = std::io::stderr();
            let mut stderr = stderr.lock();

            writeln!(
                stderr,
                "{}: {}",
                stdout.styled("Error parsing user language config", ColorSpec::BRIGHT_RED),
                err
            )?;
            writeln!(
                stderr,
                "{}",
                stdout.styled("Using default language config", ColorSpec::BRIGHT_YELLOW)
            )?;
            default_lang_config()
        }
    };

    let lang = match syn_loader_conf
        .language
        .iter()
        .find(|l| l.language_id == lang_str)
    {
        Some(l) => l,
        None => {
            let msg = format!("Language '{}' not found", lang_str);
            writeln!(stdout, "{}", stdout.styled(msg, ColorSpec::BRIGHT_RED))?;
            let suggestions: Vec<&str> = syn_loader_conf
                .language
                .iter()
                .filter(|l| {
                    lang_str
                        .chars()
                        .next()
                        .is_some_and(|first| l.language_id.starts_with(first))
                })
                .map(|l| l.language_id.as_str())
                .collect();
            if !suggestions.is_empty() {
                let suggestions = suggestions.join(", ");
                writeln!(
                    stdout,
                    "Did you mean one of these: {} ?",
                    stdout.styled(suggestions, ColorSpec::BRIGHT_YELLOW)
                )?;
            }
            return Ok(());
        }
    };

    stdout.heading(&format!("Language: {lang_str}"))?;
    writeln!(stdout)?;
    let editor_config =
        Config::load_for(&workspace::ResolvedWorkspace::current()).unwrap_or_default();
    probe_language_servers(
        stdout,
        lang,
        &syn_loader_conf.language_server,
        &editor_config.editor.lsp.workspace_roots,
    )?;

    probes::language_servers(
        stdout,
        lang,
        &syn_loader_conf.language_server,
        &editor_config.editor.lsp,
    )
    .await?;
    writeln!(stdout)?;
    probes::debug_adapter(stdout, lang.debugger.as_ref()).await?;
    writeln!(stdout)?;

    probe_protocol(
        stdout,
        "formatter",
        lang.formatter
            .as_ref()
            .map(|formatter| (formatter.command.as_str(), formatter.args.as_slice())),
    )?;

    writeln!(stdout)?;
    stdout.heading("Tree-sitter")?;
    probe_parser(stdout, lang.grammar.as_ref().unwrap_or(&lang.language_id))?;

    for (feature, source) in LanguageSupport::inspect(&Resources::default(), &lang_str).queries {
        let (status, state) = if source.is_readable() {
            (Status::Ok, "queries found")
        } else {
            (Status::Inactive, "queries missing")
        };
        stdout.row(status, feature.short_title(), state)?;
    }

    Ok(())
}

fn terminal_columns() -> usize {
    termina::PlatformTerminal::new()
        .and_then(|terminal| terminal.get_dimensions())
        .map(|size| size.cols as usize)
        .unwrap_or(80)
}

fn probe_parser(stdout: &mut Output<impl Write>, grammar_name: &str) -> std::io::Result<()> {
    let (status, state) = match loader::grammar::get_language(grammar_name) {
        Ok(Some(_)) => (Status::Ok, "loaded"),
        Ok(None) | Err(_) => (Status::Warning, "unavailable"),
    };
    stdout.row(status, "parser", state)
}

/// Describe selection without launching processes or claiming initialization succeeded.
fn probe_language_servers(
    stdout: &mut Output<impl Write>,
    language: &language_config::LanguageConfiguration,
    definitions: &std::collections::HashMap<String, language_config::LanguageServerConfiguration>,
    root_dirs: &[std::path::PathBuf],
) -> std::io::Result<()> {
    stdout.heading("Configured language servers")?;
    if language.language_servers.is_empty() {
        stdout.row(Status::Inactive, "servers", "not configured")?;
        return Ok(());
    }
    let (root, _) = view::language_servers::language_server_root(language, None, root_dirs);
    if !stdout.options.summary {
        stdout.row(Status::Info, "project root", root.display())?;
    }
    for group in language.language_server_groups() {
        let alternatives = group[0].alternative_group.is_some();
        if alternatives {
            stdout
                .note("Alternatives (first applicable executable; initialization not tested):")?;
        }
        let mut selected = false;
        for server in group {
            let Some(definition) = definitions.get(&server.name) else {
                stdout.row(Status::Error, &server.name, "definition missing")?;
                continue;
            };
            let path = match stdx::env::which(&definition.command) {
                Ok(path) => path,
                Err(_) => {
                    stdout.row(
                        Status::Error,
                        &server.name,
                        format_args!("'{}' not found in $PATH", definition.command),
                    )?;
                    if !definition.args.is_empty() {
                        stdout.field("Arguments:", format_args!("{:?}", definition.args))?;
                    }
                    continue;
                }
            };
            match view::language_servers::has_required_root(definition, &root) {
                Ok(true) => {}
                Ok(false) => {
                    stdout.row(
                        Status::Inactive,
                        &server.name,
                        "project does not match required-root-patterns",
                    )?;
                    continue;
                }
                Err(error) => {
                    stdout.row(
                        Status::Error,
                        &server.name,
                        format_args!("cannot check project requirements: {error}"),
                    )?;
                    continue;
                }
            }
            let role = if alternatives {
                if selected {
                    "fallback"
                } else {
                    "preferred candidate"
                }
            } else {
                "enabled alongside other entries"
            };
            selected = true;
            stdout.row(
                Status::Ok,
                &server.name,
                if stdout.options.summary {
                    role.to_owned()
                } else {
                    path.display().to_string()
                },
            )?;
            stdout.detail(role)?;
            if !definition.args.is_empty() {
                stdout.field("Arguments:", format_args!("{:?}", definition.args))?;
            }
        }
    }
    Ok(())
}

/// Display diagnostics about LSP and DAP.
fn probe_protocol(
    stdout: &mut Output<impl Write>,
    protocol_name: &str,
    server_cmd: Option<(&str, &[String])>,
) -> std::io::Result<()> {
    stdout.heading(&format!("Configured {protocol_name}"))?;
    let Some((cmd, args)) = server_cmd else {
        return stdout.row(Status::Inactive, "executable", "not configured");
    };
    match stdx::env::which(cmd) {
        Ok(path) => stdout.row(
            Status::Ok,
            "executable",
            if stdout.options.summary {
                "found".into()
            } else {
                path.display().to_string()
            },
        )?,
        Err(_) => stdout.row(
            Status::Error,
            "executable",
            format_args!("'{cmd}' not found in $PATH"),
        )?,
    }
    if !args.is_empty() {
        stdout.field("Arguments:", format_args!("{args:?}"))?;
    }

    Ok(())
}

pub async fn print_health(health_arg: Option<String>, options: Options) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut output = Output::new(stdout.lock(), options);
    match health_arg.as_deref() {
        Some("languages") => languages_selection(&mut output)?,
        Some("all-languages") => languages_all(&mut output)?,
        Some("clipboard") => clipboard(&mut output)?,
        None | Some("all") => {
            general(&mut output)?;
            writeln!(output)?;
            clipboard(&mut output)?;
            writeln!(output)?;
            if health_arg.is_some() {
                languages_all(&mut output)?;
            } else {
                languages_selection(&mut output)?;
            }
        }
        Some(lang) => {
            language(&mut output, lang.to_string()).await?;
            writeln!(output)?;
            runtime_warnings(&mut output, loader::runtime_dirs())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod language_server_tests {
    #[test]
    fn health_explains_preference_missing_binaries_project_requirements_and_supplements() {
        let executable = toml::Value::String(
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        let config: language_config::Configuration = toml::from_str(&format!(r#"
[language-server.missing]
command = "/__mitos_nonexistent_server__"
[language-server.inapplicable]
command = {executable}
required-root-patterns = ["__mitos_missing_marker__"]
[language-server.preferred]
command = {executable}
[language-server.fallback]
command = {executable}
[language-server.linter]
command = {executable}
[[language]]
name = "health-test"
scope = "source.health-test"
file-types = []
language-servers = [{{ alternatives = ["missing", "inapplicable", "preferred", "fallback"] }}, "linter"]
"#)).unwrap();
        let mut output = super::Output::new(
            Vec::new(),
            super::Options {
                no_color: true,
                ..Default::default()
            },
        );
        super::probe_language_servers(
            &mut output,
            &config.language[0],
            &config.language_server,
            &[],
        )
        .unwrap();
        let output = String::from_utf8(output.into_inner()).unwrap();
        assert!(output.contains("initialization not tested"));
        assert!(output.contains("not found in $PATH"));
        assert!(output.contains("project does not match required-root-patterns"));
        assert!(output.contains("preferred candidate"));
        assert!(output.contains("fallback"));
        assert!(output.contains("enabled alongside other entries"));
    }
}

#[cfg(test)]
mod runtime_warning_tests {
    #[test]
    fn query_files_and_grammar_sources_do_not_hide_missing_compiled_grammars() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("queries/bash")).unwrap();
        std::fs::write(root.path().join("queries/bash/highlights.scm"), "").unwrap();
        std::fs::create_dir_all(root.path().join("grammars/sources")).unwrap();
        // A directory with a library extension is not a compiled grammar either.
        std::fs::create_dir(root.path().join(format!(
            "grammars/bash.{}",
            loader::grammar::DYLIB_EXTENSION
        )))
        .unwrap();
        let mut output = super::Output::new(
            Vec::new(),
            super::Options {
                no_color: true,
                ..Default::default()
            },
        );
        super::runtime_warnings(&mut output, &[root.path().to_owned()]).unwrap();
        let output = String::from_utf8(output.into_inner()).unwrap();
        assert!(output.contains("No compiled tree-sitter grammars found"));
        assert!(output.contains("fetch and compile grammars before installing the runtime"));
    }

    #[test]
    fn compiled_grammar_in_lower_priority_runtime_suppresses_warning() {
        let root = tempfile::tempdir().unwrap();
        let high = root.path().join("high");
        let low = root.path().join("low");
        std::fs::create_dir(&high).unwrap();
        std::fs::create_dir_all(low.join("grammars")).unwrap();
        std::fs::write(
            low.join(format!(
                "grammars/bash.{}",
                loader::grammar::DYLIB_EXTENSION
            )),
            "library",
        )
        .unwrap();
        let mut output = super::Output::new(
            Vec::new(),
            super::Options {
                no_color: true,
                ..Default::default()
            },
        );
        super::runtime_warnings(&mut output, &[high, low]).unwrap();
        assert!(!String::from_utf8(output.into_inner())
            .unwrap()
            .contains("No compiled tree-sitter grammars found"));
    }

    #[cfg(unix)]
    #[test]
    fn compiled_grammars_are_found_through_runtime_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("system-runtime");
        std::fs::create_dir_all(runtime.join("grammars")).unwrap();
        std::fs::write(
            runtime.join(format!(
                "grammars/bash.{}",
                loader::grammar::DYLIB_EXTENSION
            )),
            "library",
        )
        .unwrap();
        let linked = root.path().join("user-runtime");
        std::os::unix::fs::symlink(&runtime, &linked).unwrap();
        let mut output = super::Output::new(
            Vec::new(),
            super::Options {
                no_color: true,
                ..Default::default()
            },
        );
        super::runtime_warnings(&mut output, &[linked]).unwrap();
        let output = String::from_utf8(output.into_inner()).unwrap();
        assert!(output.contains("is symlinked to"));
        assert!(!output.contains("No compiled tree-sitter grammars found"));
    }

    #[test]
    fn missing_and_empty_runtime_directories_are_explained() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing");
        let empty = root.path().join("empty");
        let populated = root.path().join("populated");
        std::fs::create_dir(&empty).unwrap();
        std::fs::create_dir(&populated).unwrap();
        std::fs::write(populated.join("file"), "data").unwrap();
        let mut output = super::Output::new(
            Vec::new(),
            super::Options {
                no_color: true,
                ..Default::default()
            },
        );
        super::runtime_warnings(
            &mut output,
            &[missing.clone(), empty.clone(), populated.clone()],
        )
        .unwrap();
        let output = String::from_utf8(output.into_inner()).unwrap();
        assert!(output.contains(&format!(
            "Runtime directory does not exist: {}",
            missing.display()
        )));
        assert!(output.contains(&format!("Runtime directory is empty: {}", empty.display())));
        assert!(!output.contains(&populated.to_string_lossy().to_string()));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn language_server_health_includes_configured_arguments() {
        let language = toml::from_str(
            "name = 'health-test'\nscope = 'source.health-test'\nfile-types = []\nlanguage-servers = ['wrapper']",
        )
        .unwrap();
        let definitions = toml::from_str(
            "[wrapper]\ncommand = 'missing-mitos-health-tool'\nargs = ['run', 'server', '--stdio']",
        )
        .unwrap();
        let mut output = super::Output::new(
            Vec::new(),
            super::Options {
                no_color: true,
                ..Default::default()
            },
        );
        super::probe_language_servers(&mut output, &language, &definitions, &[]).unwrap();
        let output = String::from_utf8(output.into_inner()).unwrap();
        assert!(output.contains("wrapper"));
        assert!(output.contains("Arguments: [\"run\", \"server\", \"--stdio\"]"));
    }

    #[test]
    fn formatter_health_includes_literal_wrapper_arguments() {
        let args = vec![
            "run".to_owned(),
            "black".to_owned(),
            "%{buffer_name}".to_owned(),
        ];
        let mut output = super::Output::new(
            Vec::new(),
            super::Options {
                no_color: true,
                ..Default::default()
            },
        );
        super::probe_protocol(
            &mut output,
            "formatter",
            Some(("missing-mitos-health-tool", &args)),
        )
        .unwrap();
        let output = String::from_utf8(output.into_inner()).unwrap();
        assert!(output.contains("Arguments: [\"run\", \"black\", \"%{buffer_name}\"]"));
    }
}
