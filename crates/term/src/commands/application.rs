//! Terminal application lifecycle and utility commands.

use crate::commands::context::Context;

pub(super) fn no_op(_cx: &mut Context) {}

pub(super) fn suspend(_cx: &mut Context) {
    #[cfg(not(windows))]
    {
        // SAFETY: These are calls to standard POSIX functions.
        // Unsafe is necessary since we are calling outside of Rust.
        let is_session_leader = unsafe { libc::getpid() == libc::getsid(0) };

        // If mitos is the session leader, there is nothing to suspend to, so skip
        if is_session_leader {
            return;
        }
        _cx.block_try_flush_writes().ok();
        signal_hook::low_level::raise(signal_hook::consts::signal::SIGTSTP).unwrap();
    }
}

pub(super) mod typed {
    //! Typable application commands.

    use crate::{
        commands::{
            buffers::typed::buffers_remaining_impl,
            catalog::{WRITE_NO_CODE_ACTIONS_FLAG, WRITE_NO_FORMAT_FLAG},
            files::typed::{write_all_impl, write_impl, WriteAllOptions, WriteOptions},
        },
        compositor, job,
        ui::PromptEvent,
    };
    use ::command_line::Args;
    use view::editor::Action;

    #[cold]
    pub(in crate::commands) fn exit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        if doc!(cx.editor).is_modified() {
            write_impl(
                cx,
                args.first(),
                WriteOptions {
                    force: false,
                    auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                    code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
                },
            )?;
        }
        cx.block_try_flush_writes()?;
        quit(cx, Args::default(), event)
    }

    #[cold]
    pub(in crate::commands) fn force_exit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        if doc!(cx.editor).is_modified() {
            write_impl(
                cx,
                args.first(),
                WriteOptions {
                    force: true,
                    auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                    code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
                },
            )?;
        }
        cx.block_try_flush_writes()?;
        quit(cx, Args::default(), event)
    }

    #[cold]
    pub(in crate::commands) fn quit(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        log::debug!("quitting...");

        if event != PromptEvent::Validate {
            return Ok(());
        }

        // last view and we have unsaved changes
        if cx.editor.tree.views().count() == 1 {
            buffers_remaining_impl(cx.editor)?
        }

        cx.block_try_flush_writes()?;
        cx.editor.close(view!(cx.editor).id);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn force_quit(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        cx.block_try_flush_writes()?;
        cx.editor.close(view!(cx.editor).id);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn write_quit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        write_impl(
            cx,
            args.first(),
            WriteOptions {
                force: false,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )?;
        cx.block_try_flush_writes()?;
        quit(cx, Args::default(), event)
    }

    #[cold]
    pub(in crate::commands) fn force_write_quit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        write_impl(
            cx,
            args.first(),
            WriteOptions {
                force: true,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )?;
        cx.block_try_flush_writes()?;
        force_quit(cx, Args::default(), event)
    }

    #[cold]
    pub(in crate::commands) fn write_all_quit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }
        write_all_impl(
            cx.editor,
            cx.jobs,
            WriteAllOptions {
                force: false,
                write_scratch: true,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )?;
        quit_all_impl(cx, false)
    }

    #[cold]
    pub(in crate::commands) fn force_write_all_quit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }
        let _ = write_all_impl(
            cx.editor,
            cx.jobs,
            WriteAllOptions {
                force: true,
                write_scratch: true,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        );
        quit_all_impl(cx, true)
    }

    fn quit_all_impl(cx: &mut compositor::Context, force: bool) -> anyhow::Result<()> {
        cx.block_try_flush_writes()?;
        if !force {
            buffers_remaining_impl(cx.editor)?;
        }

        // close all views
        let views: Vec<_> = cx.editor.tree.views().map(|(view, _)| view.id).collect();
        for view_id in views {
            cx.editor.close(view_id);
        }

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn quit_all(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        quit_all_impl(cx, false)
    }

    #[cold]
    pub(in crate::commands) fn force_quit_all(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        quit_all_impl(cx, true)
    }

    #[cold]
    pub(in crate::commands) fn cquit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let exit_code = args
            .first()
            .and_then(|code| code.parse::<i32>().ok())
            .unwrap_or(1);

        cx.editor.exit_code = exit_code;
        quit_all_impl(cx, false)
    }

    #[cold]
    pub(in crate::commands) fn force_cquit(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let exit_code = args
            .first()
            .and_then(|code| code.parse::<i32>().ok())
            .unwrap_or(1);
        cx.editor.exit_code = exit_code;

        quit_all_impl(cx, true)
    }

    #[cold]
    pub(in crate::commands) fn open_log(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        cx.editor.open(&loader::log_file(), Action::Replace)?;
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn redraw(
        cx: &mut compositor::Context,
        _args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let callback = Box::pin(async move {
            let call: job::Callback =
                job::Callback::EditorCompositor(Box::new(|_editor, compositor| {
                    compositor.need_full_redraw();
                }));

            Ok(call)
        });

        cx.jobs.callback(callback);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn echo(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let output = args.into_iter().fold(String::new(), |mut acc, arg| {
            if !acc.is_empty() {
                acc.push(' ');
            }
            acc.push_str(&arg);
            acc
        });
        cx.editor.set_status(output);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn noop(
        _cx: &mut compositor::Context,
        _args: Args,
        _event: PromptEvent,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
