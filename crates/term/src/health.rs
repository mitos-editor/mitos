use crate::config::{Config, ConfigLoadError};
use language::loader::{default_lang_config, user_lang_config};
use loader::grammar::load_runtime_file;
use std::{
    collections::HashSet,
    io::{IsTerminal, Write},
};
use termina::{
    style::{ColorSpec, StyleExt as _, Stylized},
    Terminal as _,
};

#[derive(Copy, Clone)]
pub enum TsFeature {
    Highlight,
    TextObject,
    AutoIndent,
    Tags,
    RainbowBracket,
}

impl TsFeature {
    pub fn all() -> &'static [Self] {
        &[
            Self::Highlight,
            Self::TextObject,
            Self::AutoIndent,
            Self::Tags,
            Self::RainbowBracket,
        ]
    }

    pub fn runtime_filename(&self) -> &'static str {
        match *self {
            Self::Highlight => "highlights.scm",
            Self::TextObject => "textobjects.scm",
            Self::AutoIndent => "indents.scm",
            Self::Tags => "tags.scm",
            Self::RainbowBracket => "rainbows.scm",
        }
    }

    pub fn long_title(&self) -> &'static str {
        match *self {
            Self::Highlight => "Syntax Highlighting",
            Self::TextObject => "Treesitter Textobjects",
            Self::AutoIndent => "Auto Indent",
            Self::Tags => "Code Navigation Tags",
            Self::RainbowBracket => "Rainbow Brackets",
        }
    }

    pub fn short_title(&self) -> &'static str {
        match *self {
            Self::Highlight => "Highlight",
            Self::TextObject => "Textobject",
            Self::AutoIndent => "Indent",
            Self::Tags => "Tags",
            Self::RainbowBracket => "Rainbow",
        }
    }
}

/// Display general diagnostics.
pub fn general() -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    let config_file = loader::config_file();
    let lang_file = loader::lang_config_file();
    let log_file = loader::log_file();
    let rt_dirs = loader::runtime_dirs();

    if config_file.exists() {
        writeln!(stdout, "Config file: {}", config_file.display())?;
    } else {
        writeln!(stdout, "Config file: default")?;
    }
    if lang_file.exists() {
        writeln!(stdout, "Language file: {}", lang_file.display())?;
    } else {
        writeln!(stdout, "Language file: default")?;
    }
    writeln!(stdout, "Log file: {}", log_file.display())?;
    writeln!(
        stdout,
        "Runtime directories: {}",
        rt_dirs
            .iter()
            .map(|d| d.to_string_lossy())
            .collect::<Vec<_>>()
            .join(";")
    )?;
    for rt_dir in rt_dirs.iter() {
        if let Ok(path) = std::fs::read_link(rt_dir) {
            let msg = format!(
                "Runtime directory {} is symlinked to: {}",
                rt_dir.display(),
                path.display()
            );
            writeln!(stdout, "{}", msg.yellow())?;
        }
        if !rt_dir.exists() {
            let msg = format!("Runtime directory does not exist: {}", rt_dir.display());
            writeln!(stdout, "{}", msg.yellow())?;
        } else if rt_dir.read_dir().ok().map(|it| it.count()) == Some(0) {
            let msg = format!("Runtime directory is empty: {}", rt_dir.display());
            writeln!(stdout, "{}", msg.yellow())?;
        }
    }

    Ok(())
}

pub fn clipboard() -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    let config = match Config::load_default() {
        Ok(config) => config,
        Err(ConfigLoadError::Error(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            Config::default()
        }
        Err(err) => {
            writeln!(stdout, "{}", "Configuration file malformed".red())?;
            writeln!(stdout, "{}", err)?;
            return Ok(());
        }
    };

    use ::clipboard::ClipboardBackend;
    match crate::clipboard::TerminalClipboard
        .name(&config.editor.clipboard.provider)
        .as_str()
    {
        "none" => {
            writeln!(
                stdout,
                "{}",
                "System clipboard provider: Not installed".red()
            )?;
            writeln!(
                stdout,
                "    {}",
                "For troubleshooting system clipboard issues, refer".red()
            )?;
            writeln!(stdout, "    {}",
                "https://github.com/helix-editor/helix/wiki/Troubleshooting#copypaste-fromto-system-clipboard-not-working"
            .red().underlined())?;
        }
        name => writeln!(stdout, "System clipboard provider: {}", name)?,
    }

    Ok(())
}

pub fn languages_all() -> std::io::Result<()> {
    languages(None)
}

pub fn languages_selection() -> std::io::Result<()> {
    let selection = loader::grammar::get_grammar_names().unwrap_or_default();
    languages(selection)
}

fn languages(selection: Option<HashSet<String>>) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    let trust = workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir());
    let mut syn_loader_conf = match user_lang_config(&trust) {
        Ok(conf) => conf,
        Err(err) => {
            let stderr = std::io::stderr();
            let mut stderr = stderr.lock();

            writeln!(
                stderr,
                "{}: {}",
                "Error parsing user language config".red(),
                err
            )?;
            writeln!(stderr, "{}", "Using default language config".yellow())?;
            default_lang_config()
        }
    };

    let mut headings = vec!["Language", "Language servers", "Debug adapter", "Formatter"];

    for feat in TsFeature::all() {
        headings.push(feat.short_title())
    }

    let terminal_cols = termina::PlatformTerminal::new()
        .and_then(|terminal| terminal.get_dimensions())
        .map(|size| size.cols)
        .unwrap_or(80);
    let column_width = terminal_cols as usize / headings.len();
    let is_terminal = std::io::stdout().is_terminal();

    let fit = |s: &str| -> Stylized<'static> {
        format!(
            "{:column_width$}",
            s.get(..column_width - 2)
                .map(|s| format!("{}…", s))
                .unwrap_or_else(|| s.to_string())
        )
        .stylized()
    };
    let color = |s: Stylized<'static>, c: ColorSpec| if is_terminal { s.foreground(c) } else { s };
    let bold = |s: Stylized<'static>| if is_terminal { s.bold() } else { s };

    for heading in headings {
        write!(stdout, "{}", bold(fit(heading)))?;
    }
    writeln!(stdout)?;

    syn_loader_conf
        .language
        .sort_unstable_by_key(|l| l.language_id.clone());

    let check_binary_with_name = |cmd: Option<(&str, &str)>| match cmd {
        Some((name, cmd)) => match stdx::env::which(cmd) {
            Ok(_) => color(fit(&format!("✓ {}", name)), ColorSpec::BRIGHT_GREEN),
            Err(_) => color(fit(&format!("✘ {}", name)), ColorSpec::BRIGHT_RED),
        },
        None => color(fit("None"), ColorSpec::BRIGHT_YELLOW),
    };

    let check_binary = |cmd: Option<&str>| check_binary_with_name(cmd.map(|cmd| (cmd, cmd)));

    for lang in &syn_loader_conf.language {
        if selection
            .as_ref()
            .is_some_and(|s| !s.contains(&lang.language_id))
        {
            continue;
        }

        write!(stdout, "{}", fit(&lang.language_id))?;

        let mut cmds = lang.language_servers.iter().filter_map(|ls| {
            syn_loader_conf
                .language_server
                .get(&ls.name)
                .map(|config| (ls.name.as_str(), config.command.as_str()))
        });
        write!(stdout, "{}", check_binary_with_name(cmds.next()))?;

        let dap = lang.debugger.as_ref().map(|dap| dap.command.as_str());
        write!(stdout, "{}", check_binary(dap))?;

        let formatter = lang
            .formatter
            .as_ref()
            .map(|formatter| formatter.command.as_str());
        write!(stdout, "{}", check_binary(formatter))?;

        for ts_feat in TsFeature::all() {
            match load_runtime_file(&lang.language_id, ts_feat.runtime_filename()).is_ok() {
                true => write!(stdout, "{}", color(fit("✓"), ColorSpec::BRIGHT_GREEN))?,
                false => write!(stdout, "{}", color(fit("✘"), ColorSpec::BRIGHT_RED))?,
            }
        }

        writeln!(stdout)?;

        for cmd in cmds {
            write!(stdout, "{}", fit(""))?;
            writeln!(stdout, "{}", check_binary_with_name(Some(cmd)))?;
        }
    }

    if selection.is_some() {
        writeln!(
            stdout,
            "\nThis list is filtered according to the 'use-grammars' option in languages.toml file.\n\
            To see the full list, use the '--health all' or '--health all-languages' option."
        )?;
    }

    Ok(())
}

/// Display diagnostics pertaining to a particular language (LSP,
/// highlight queries, etc).
pub fn language(lang_str: String) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    let trust = workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir());
    let syn_loader_conf = match user_lang_config(&trust) {
        Ok(conf) => conf,
        Err(err) => {
            let stderr = std::io::stderr();
            let mut stderr = stderr.lock();

            writeln!(
                stderr,
                "{}: {}",
                "Error parsing user language config".red(),
                err
            )?;
            writeln!(stderr, "{}", "Using default language config".yellow())?;
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
            writeln!(stdout, "{}", msg.red())?;
            let suggestions: Vec<&str> = syn_loader_conf
                .language
                .iter()
                .filter(|l| l.language_id.starts_with(lang_str.chars().next().unwrap()))
                .map(|l| l.language_id.as_str())
                .collect();
            if !suggestions.is_empty() {
                let suggestions = suggestions.join(", ");
                writeln!(
                    stdout,
                    "Did you mean one of these: {} ?",
                    suggestions.yellow()
                )?;
            }
            return Ok(());
        }
    };

    let editor_config = Config::load_default().unwrap_or_default();
    probe_language_servers(
        &mut stdout,
        lang,
        &syn_loader_conf.language_server,
        &editor_config.editor.lsp.workspace_roots,
    )?;

    probe_protocol(
        "debug adapter",
        lang.debugger.as_ref().map(|dap| dap.command.to_string()),
    )?;

    probe_protocol(
        "formatter",
        lang.formatter
            .as_ref()
            .map(|formatter| formatter.command.to_string()),
    )?;

    probe_parser(lang.grammar.as_ref().unwrap_or(&lang.language_id))?;

    for ts_feat in TsFeature::all() {
        probe_treesitter_feature(&lang_str, *ts_feat)?
    }

    Ok(())
}

fn probe_parser(grammar_name: &str) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    write!(stdout, "Tree-sitter parser: ")?;

    match loader::grammar::get_language(grammar_name) {
        Ok(Some(_)) => writeln!(stdout, "{}", "✓".green()),
        Ok(None) | Err(_) => writeln!(stdout, "{}", "None".yellow()),
    }
}

/// Describe selection without launching processes or claiming initialization succeeded.
fn probe_language_servers(
    stdout: &mut impl Write,
    language: &language::config::LanguageConfiguration,
    definitions: &std::collections::HashMap<String, language::config::LanguageServerConfiguration>,
    root_dirs: &[std::path::PathBuf],
) -> std::io::Result<()> {
    writeln!(stdout, "Configured language servers:")?;
    if language.language_servers.is_empty() {
        writeln!(stdout, "  None")?;
        return Ok(());
    }
    let (root, _) = lsp_client::language_server_root(language, None, root_dirs);
    writeln!(stdout, "  Project root: {}", root.display())?;
    for group in language.language_server_groups() {
        let alternatives = group[0].alternative_group.is_some();
        if alternatives {
            writeln!(
                stdout,
                "  Alternatives (first applicable executable; initialization not tested):"
            )?;
        }
        let mut selected = false;
        for server in group {
            let Some(definition) = definitions.get(&server.name) else {
                writeln!(stdout, "    ✘ {}: definition missing", server.name)?;
                continue;
            };
            let path = match stdx::env::which(&definition.command) {
                Ok(path) => path,
                Err(_) => {
                    writeln!(
                        stdout,
                        "    ✘ {}: '{}' not found in $PATH",
                        server.name, definition.command
                    )?;
                    continue;
                }
            };
            match lsp_client::has_required_root(definition, &root) {
                Ok(true) => {}
                Ok(false) => {
                    writeln!(
                        stdout,
                        "    − {}: project does not match required-root-patterns",
                        server.name
                    )?;
                    continue;
                }
                Err(error) => {
                    writeln!(
                        stdout,
                        "    ✘ {}: cannot check project requirements: {error}",
                        server.name
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
            writeln!(stdout, "    ✓ {}: {} ({role})", server.name, path.display())?;
        }
    }
    Ok(())
}

/// Display diagnostics about LSP and DAP.
fn probe_protocol(protocol_name: &str, server_cmd: Option<String>) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    write!(stdout, "Configured {}:", protocol_name)?;
    let Some(cmd) = server_cmd else {
        writeln!(stdout, "{}", " None".yellow())?;
        return Ok(());
    };
    writeln!(stdout)?;

    let (diag, icon) = match stdx::env::which(&cmd) {
        Ok(path) => (path.display().to_string().green(), "✓".green()),
        Err(_) => (format!("'{}' not found in $PATH", cmd).red(), "✘".red()),
    };
    writeln!(stdout, "  {} {}", icon, diag)?;

    Ok(())
}

/// Display diagnostics about a feature that requires tree-sitter
/// query files (highlights, textobjects, etc).
fn probe_treesitter_feature(lang: &str, feature: TsFeature) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    let found = match load_runtime_file(lang, feature.runtime_filename()).is_ok() {
        true => "✓".green(),
        false => "✘".red(),
    };
    writeln!(stdout, "{} queries: {}", feature.short_title(), found)?;

    Ok(())
}

pub fn print_health(health_arg: Option<String>) -> std::io::Result<()> {
    match health_arg.as_deref() {
        Some("languages") => languages_selection()?,
        Some("all-languages") => languages_all()?,
        Some("clipboard") => clipboard()?,
        None => {
            general()?;
            clipboard()?;
            writeln!(std::io::stdout().lock())?;
            languages_selection()?;
        }
        Some("all") => {
            general()?;
            clipboard()?;
            writeln!(std::io::stdout().lock())?;
            languages_all()?;
        }
        Some(lang) => language(lang.to_string())?,
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
        let config: language::config::Configuration = toml::from_str(&format!(r#"
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
        let mut output = Vec::new();
        super::probe_language_servers(
            &mut output,
            &config.language[0],
            &config.language_server,
            &[],
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("initialization not tested"));
        assert!(output.contains("not found in $PATH"));
        assert!(output.contains("project does not match required-root-patterns"));
        assert!(output.contains("preferred candidate"));
        assert!(output.contains("(fallback)"));
        assert!(output.contains("enabled alongside other entries"));
    }
}
