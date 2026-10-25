use super::Fixture;
use command_line::Token;
use view::{current, expansion::Context};

#[tokio::test]
async fn expansions_use_captured_variables_and_register_values() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("first\n")?;
    fixture.editor.registers.write('a', vec!["before".into()])?;
    let source = "%{selection}:%reg{a}";
    let context = Context::capture(&fixture.editor, source);
    fixture.editor.registers.write('a', vec!["after".into()])?;
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, editor_core::Selection::single(1, 2));
    assert_eq!(
        context.expand(Token::expand(source), &[]).await?,
        "f:before"
    );
    Ok(())
}

#[tokio::test]
async fn empty_nested_expansions_remove_their_source_text() -> anyhow::Result<()> {
    let fixture = Fixture::new("text\n")?;
    let source = "%reg{z}";
    let context = Context::capture(&fixture.editor, source);
    assert_eq!(context.expand(Token::expand(source), &[]).await?, "");
    let source = "%%reg{z}";
    let context = Context::capture(&fixture.editor, source);
    assert_eq!(context.expand(Token::expand(source), &[]).await?, "%reg{z}");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn nested_shell_expansions_use_captured_values() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("text\n")?;
    fixture.editor.registers.write('a', vec!["before".into()])?;
    let source = "%sh{printf '%sh{printf %reg{a}}'}";
    let context = Context::capture(&fixture.editor, source);
    fixture.editor.registers.write('a', vec!["after".into()])?;
    assert_eq!(context.expand(Token::expand(source), &[]).await?, "before");
    Ok(())
}

#[tokio::test]
async fn interactive_expansions_require_a_frontend_executor() -> anyhow::Result<()> {
    let fixture = Fixture::new("text\n")?;
    let source = "%sh_interactive{echo %{cursor_line}}";
    let context = Context::capture(&fixture.editor, source);
    assert!(context
        .expand(Token::expand(source), &[])
        .await
        .unwrap_err()
        .to_string()
        .contains("only available in submitted editor commands"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn interactive_expansions_compose_with_captured_values_and_do_not_reexpand_output(
) -> anyhow::Result<()> {
    use futures_util::FutureExt;
    let mut fixture = Fixture::new("text\n")?;
    fixture.editor.registers.write('a', vec!["before".into()])?;
    let source = "%sh_interactive{printf '%reg{a}:%{cursor_line}:%%%%{cursor_line}\\n'}";
    let context = Context::capture(&fixture.editor, source).with_interactive_shell(
        std::sync::Arc::new(|mut command| async move { Ok(command.output().await?) }.boxed()),
    );
    fixture.editor.registers.write('a', vec!["after".into()])?;
    assert_eq!(
        context.expand(Token::expand(source), &[]).await?,
        "before:1:%{cursor_line}"
    );
    Ok(())
}
