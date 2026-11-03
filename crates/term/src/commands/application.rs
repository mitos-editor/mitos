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
        let _ = _cx.jobs.after_writes(
            _cx.editor,
            Box::new(|_| {
                signal_hook::low_level::raise(signal_hook::consts::signal::SIGTSTP)?;
                Ok(())
            }),
        );
    }
}

pub(super) mod typed {
    //! Typable application commands.

    use crate::{
        commands::{
            buffers::typed::buffers_remaining_impl,
            catalog::{WRITE_NO_CODE_ACTIONS_FLAG, WRITE_NO_FORMAT_FLAG},
            files::typed::{write_all_impl, write_impl},
        },
        compositor, job,
    };
    use ::command_line::Args;
    use view::{
        editor::Action,
        save::{WriteAllOptions, WriteOptions},
    };

    #[cold]
    pub(in crate::commands) fn exit<const FORCE: bool>(
        cx: &mut compositor::Context,
        args: Args,
    ) -> anyhow::Result<()> {
        if doc!(cx.editor).is_modified() {
            write_impl(
                cx,
                args.first(),
                WriteOptions {
                    force: FORCE,
                    auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                    code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
                },
            )?;
        }
        quit::<false>(cx, Args::default())
    }

    #[cold]
    pub(in crate::commands) fn quit<const FORCE: bool>(
        cx: &mut compositor::Context,
        _args: Args,
    ) -> anyhow::Result<()> {
        log::debug!("quitting...");
        let view = view!(cx.editor);
        let (view_id, binding) = (view.id, view.binding_revision());
        cx.after_writes(move |editor| {
            if editor
                .tree
                .try_get(view_id)
                .is_none_or(|view| view.binding_revision() != binding)
            {
                return Ok(());
            }
            if !FORCE && editor.tree.views().count() == 1 {
                buffers_remaining_impl(editor)?;
            }
            editor.close(view_id);
            Ok(())
        })
    }

    #[cold]
    pub(in crate::commands) fn write_quit(
        cx: &mut compositor::Context,
        args: Args,
    ) -> anyhow::Result<()> {
        write_impl(
            cx,
            args.first(),
            WriteOptions {
                force: false,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )?;
        quit::<false>(cx, Args::default())
    }

    #[cold]
    pub(in crate::commands) fn force_write_quit(
        cx: &mut compositor::Context,
        args: Args,
    ) -> anyhow::Result<()> {
        write_impl(
            cx,
            args.first(),
            WriteOptions {
                force: true,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )?;
        let view = view!(cx.editor);
        let (view_id, binding) = (view.id, view.binding_revision());
        let doc_id = doc!(cx.editor).id();
        cx.after_writes(move |editor| {
            if editor
                .tree
                .try_get(view_id)
                .is_none_or(|view| view.binding_revision() != binding)
            {
                return Ok(());
            }
            if editor.document(doc_id).is_some_and(|doc| doc.is_modified()) {
                anyhow::bail!("Buffer changed while saving; write again before quitting");
            }
            if editor.tree.contains(view_id) {
                editor.close(view_id);
            }
            Ok(())
        })
    }

    #[cold]
    pub(in crate::commands) fn write_all_quit<const FORCE: bool>(
        cx: &mut compositor::Context,
        args: Args,
    ) -> anyhow::Result<()> {
        write_all_impl(
            cx.editor,
            WriteAllOptions {
                force: FORCE,
                write_scratch: true,
                auto_format: !args.has_flag(WRITE_NO_FORMAT_FLAG.name),
                code_actions: !args.has_flag(WRITE_NO_CODE_ACTIONS_FLAG.name),
            },
        )?;
        quit_all::<false>(cx, Args::default())
    }

    #[cold]
    pub(in crate::commands) fn quit_all<const FORCE: bool>(
        cx: &mut compositor::Context,
        _args: Args,
    ) -> anyhow::Result<()> {
        cx.after_writes(move |editor| {
            if !FORCE {
                buffers_remaining_impl(editor)?;
            }
            let views: Vec<_> = editor.tree.views().map(|(view, _)| view.id).collect();
            for view_id in views {
                editor.close(view_id);
            }
            Ok(())
        })
    }

    #[cold]
    pub(in crate::commands) fn cquit<const FORCE: bool>(
        cx: &mut compositor::Context,
        args: Args,
    ) -> anyhow::Result<()> {
        let exit_code = args
            .first()
            .and_then(|code| code.parse::<i32>().ok())
            .unwrap_or(1);

        cx.editor.exit_code = exit_code;
        quit_all::<FORCE>(cx, Args::default())
    }

    #[cold]
    pub(in crate::commands) fn open_log(
        cx: &mut compositor::Context,
        _args: Args,
    ) -> anyhow::Result<()> {
        cx.editor.open(&loader::log_file(), Action::Replace)?;
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn redraw(
        cx: &mut compositor::Context,
        _args: Args,
    ) -> anyhow::Result<()> {
        let callback = Box::pin(async move {
            let call: job::Callback = job::Callback::ui(move |compositor, _cx| {
                compositor.need_full_redraw();
            });

            Ok(call)
        });

        cx.jobs.callback(callback);

        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn echo(
        cx: &mut compositor::Context,
        args: Args,
    ) -> anyhow::Result<()> {
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
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
