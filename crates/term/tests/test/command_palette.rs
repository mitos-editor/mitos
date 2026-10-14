use super::helpers::*;

#[tokio::test(flavor = "multi_thread")]
async fn palette_runs_a_command_that_opens_a_prompt() -> anyhow::Result<()> {
    test(("#[a|]#bc\n", "<space>?search<ret>bc<ret>", "a#[bc|]#\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn palette_preserves_a_commands_next_key_handler() -> anyhow::Result<()> {
    test(("#[a|]#bc\n", "<space>?find_next_char<ret>c", "#[abc|]#\n")).await
}
