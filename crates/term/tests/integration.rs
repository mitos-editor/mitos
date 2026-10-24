#[cfg(feature = "integration")]
mod test {
    mod helpers;

    use editor_core::Selection;
    use language_config::AutoPairConfig;
    use term::config::Config;

    use indoc::indoc;

    use self::helpers::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn hello_world() -> anyhow::Result<()> {
        test(("#[\n|]#", "ihello world<esc>", "hello world#[|\n]#")).await?;
        Ok(())
    }

    mod auto_pairs;
    mod auto_reload;
    mod auto_save;
    mod buffer_picker;
    mod code_action_hints;
    mod code_actions;
    mod code_lenses;
    mod command_line;
    mod command_palette;
    mod commands;
    mod completion;
    mod component_keymaps;
    mod config_application;
    mod cursor;
    mod debug_mode;
    mod debugger_execution;
    mod debugger_inline_values;
    mod debugger_sessions;
    mod debugger_terminal;
    mod debugger_variables;
    mod document_features;
    mod document_symbols;
    mod file_operations;
    mod formatting;
    mod inline_completion;
    mod language_features;
    mod lsp_alternatives;
    mod lsp_lifecycle;
    mod lsp_workspace;
    mod movement;
    mod navigation;
    mod picker_layout;
    mod pull_diagnostics;
    mod quicklist;
    mod rename;
    mod rulers;
    mod signature_help;
    mod spelling;
    mod splits;
    mod startup;
    mod statusline;
    mod user_snippets;
    mod workspace_symbols;
    mod workspace_trust;
}
