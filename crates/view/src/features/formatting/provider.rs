//! Formatter execution and provider selection for explicit formatting and saves.
use std::{fmt::Display, path::Path};

use command_line::Token;
use editor_core::{
    encoding, indent::IndentStyle, syntax::config::LanguageServerFeature, Rope, Transaction,
};
use futures_util::{future::BoxFuture, FutureExt};
use lsp_client::lsp;

use crate::{document::to_writer, expansion, Document, Editor};

impl Document {
    /// The same as [`Self::format`], but only returns formatting changes if auto-formatting
    /// is configured.
    pub fn auto_format(
        &self,
        editor: &Editor,
    ) -> Option<BoxFuture<'static, Result<Transaction, FormatterError>>> {
        if self
            .language_config()?
            .auto_format
            .unwrap_or(editor.config().format.on_save)
        {
            self.format(editor)
        } else {
            None
        }
    }

    /// Compute edits using the configured formatter or the first capable language server.
    pub fn format(
        &self,
        editor: &Editor,
    ) -> Option<BoxFuture<'static, Result<Transaction, FormatterError>>> {
        if let Some((fmt_cmd, fmt_args)) = self
            .language_config()
            .and_then(|c| c.formatter.as_ref())
            .and_then(|formatter| {
                Some((stdx::env::which(&formatter.command).ok()?, &formatter.args))
            })
        {
            log::debug!(
                "formatting '{}' with command '{}', args {fmt_args:?}",
                self.display_name(),
                fmt_cmd.display(),
            );
            use std::process::Stdio;
            let text = self.text().clone();

            let mut process = tokio::process::Command::new(&fmt_cmd);

            let directory = self
                .path()
                .and_then(|path| path.parent())
                .map(Path::to_path_buf)
                .unwrap_or_else(stdx::env::current_working_dir);
            process.current_dir(directory);

            let args = match fmt_args
                .iter()
                .map(|content| expansion::expand(editor, Token::expand(content), &[]))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(args) => args,
                Err(err) => {
                    log::error!("Failed to expand formatter arguments: {err}");
                    return None;
                }
            };

            process
                .args(args.iter().map(AsRef::as_ref))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let formatting_future = async move {
                let mut process = process
                    .spawn()
                    .map_err(|e| FormatterError::SpawningFailed {
                        command: fmt_cmd.to_string_lossy().into(),
                        error: e.kind(),
                    })?;

                let mut stdin = process.stdin.take().ok_or(FormatterError::BrokenStdin)?;
                let input_text = text.clone();
                let input_task = tokio::spawn(async move {
                    to_writer(&mut stdin, (encoding::UTF_8, false), &input_text).await
                    // Note that `stdin` is dropped here, causing the pipe to close. This can
                    // avoid a deadlock with `wait_with_output` below if the process is waiting on
                    // stdin to close before exiting.
                });
                let (input_result, output_result) = tokio::join! {
                    input_task,
                    process.wait_with_output(),
                };
                let _ = input_result.map_err(|_| FormatterError::BrokenStdin)?;
                let output = output_result.map_err(|_| FormatterError::WaitForOutputFailed)?;

                if !output.status.success() {
                    if !output.stderr.is_empty() {
                        let err = String::from_utf8_lossy(&output.stderr).to_string();
                        log::error!("Formatter error: {}", err);
                        return Err(FormatterError::NonZeroExitStatus(Some(err)));
                    }

                    return Err(FormatterError::NonZeroExitStatus(None));
                } else if !output.stderr.is_empty() {
                    log::debug!(
                        "Formatter printed to stderr: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }

                let str = std::str::from_utf8(&output.stdout)
                    .map_err(|_| FormatterError::InvalidUtf8Output)?;

                Ok(editor_core::diff::compare_ropes(&text, &Rope::from(str)))
            };
            return Some(formatting_future.boxed());
        };

        let text = self.text().clone();
        // finds first language server that supports formatting and then formats
        let language_server = self
            .language_servers_with_feature(LanguageServerFeature::Format)
            .next()?;
        let offset_encoding = language_server.offset_encoding();
        let request = language_server.text_document_formatting(
            self.identifier(),
            lsp::FormattingOptions {
                tab_size: self.tab_width() as u32,
                insert_spaces: matches!(self.indent_style, IndentStyle::Spaces(_)),
                ..Default::default()
            },
            None,
        )?;

        let fut = async move {
            let edits = request
                .await
                .unwrap_or_else(|e| {
                    log::warn!("LSP formatting failed: {}", e);
                    Default::default()
                })
                .unwrap_or_default();
            Ok(lsp_client::util::generate_transaction_from_edits(
                &text,
                edits,
                offset_encoding,
            ))
        };
        Some(fut.boxed())
    }
}

#[derive(Clone, Debug)]
pub enum FormatterError {
    SpawningFailed {
        command: String,
        error: std::io::ErrorKind,
    },
    BrokenStdin,
    WaitForOutputFailed,
    InvalidUtf8Output,
    NonZeroExitStatus(Option<String>),
}

impl std::error::Error for FormatterError {}

impl Display for FormatterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawningFailed { command, error } => {
                write!(f, "Failed to spawn formatter {}: {:?}", command, error)
            }
            Self::BrokenStdin => write!(f, "Could not write to formatter stdin"),
            Self::WaitForOutputFailed => write!(f, "Waiting for formatter output failed"),
            Self::InvalidUtf8Output => write!(f, "Invalid UTF-8 formatter output"),
            Self::NonZeroExitStatus(Some(output)) => write!(f, "Formatter error: {}", output),
            Self::NonZeroExitStatus(None) => {
                write!(f, "Formatter exited with non zero exit status")
            }
        }
    }
}
