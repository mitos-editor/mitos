#[cfg(feature = "integration")]
mod test {
    mod helpers;

    use editor_core::{syntax::config::AutoPairConfig, Selection};
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
    mod code_action_hints;
    mod command_line;
    mod commands;
    mod completion;
    mod config_application;
    mod document_features;
    mod lsp_lifecycle;
    mod lsp_workspace;
    mod movement;
    mod pull_diagnostics;
    mod signature_help;
    mod spelling;
    mod splits;
    mod startup;
}
