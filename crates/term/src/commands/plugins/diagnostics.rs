//! Native developer commands consume bounded owned snapshots, never guest calls.

use std::fmt::Write;

use command_line::Args;
use plugin_api::diagnostics::PluginDiagnostics;

use crate::{compositor, ui::PromptEvent};

const MAX_REPORT_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy)]
enum Report {
    Inspect,
    Logs,
    Timings,
}

pub(in crate::commands) fn inspect(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    show(cx, args, event, Report::Inspect)
}
pub(in crate::commands) fn logs(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    show(cx, args, event, Report::Logs)
}
pub(in crate::commands) fn timings(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    show(cx, args, event, Report::Timings)
}

fn show(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
    report: Report,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let snapshots = cx.editor.plugin_diagnostics();
    let filter = args.first();
    anyhow::ensure!(
        filter.is_none_or(|name| snapshots.iter().any(|plugin| plugin.plugin == name)),
        "unknown configured plugin: {}",
        filter.unwrap_or_default()
    );
    let output = render(&snapshots, filter, report);
    cx.editor.new_file(view::editor::Action::Replace);
    let view = view::view!(cx.editor).id;
    let doc = view::doc_mut!(cx.editor);
    let transaction = editor_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some(output.into()))].into_iter(),
    );
    doc.apply(&transaction, view);
    doc.reset_modified();
    Ok(())
}

fn render(snapshots: &[PluginDiagnostics], filter: Option<&str>, report: Report) -> String {
    let mut output = String::new();
    let mut snapshots: Vec<_> = snapshots
        .iter()
        .filter(|plugin| filter.is_none_or(|name| plugin.plugin == name))
        .collect();
    snapshots.sort_by(|left, right| left.plugin.cmp(&right.plugin));
    for plugin in snapshots {
        let mut section = String::new();
        let engine = match plugin.engine {
            Some(plugin_api::diagnostics::PluginEngine::Component) => "component",
            Some(plugin_api::diagnostics::PluginEngine::Declarative) => "declarative",
            None => "manifest unread",
        };
        writeln!(
            &mut section,
            "{} — {:?}, {}, generation {}",
            plugin.plugin, plugin.status, engine, plugin.generation
        )
        .unwrap();
        match report {
            Report::Inspect => {
                writeln!(
                    &mut section,
                    "Declared: {}",
                    serde_json::to_string(&plugin.declared).unwrap()
                )
                .unwrap();
                writeln!(
                    &mut section,
                    "Granted: {}",
                    serde_json::to_string(&plugin.effective).unwrap()
                )
                .unwrap();
                writeln!(
                    &mut section,
                    "Queued: {}; completed: {}; failed: {}; cancelled: {}",
                    plugin.queued,
                    plugin.timings.completed,
                    plugin.timings.failed,
                    plugin.timings.cancelled
                )
                .unwrap();
            }
            Report::Logs => {
                for entry in plugin
                    .entries
                    .iter()
                    .rev()
                    .take(plugin_api::diagnostics::MAX_DIAGNOSTIC_ENTRIES)
                {
                    let safe = plugin_api::diagnostics::DiagnosticEntry::bounded(
                        entry.sequence,
                        entry.level,
                        &entry.message,
                    );
                    writeln!(
                        &mut section,
                        "{} {:?}: {}",
                        safe.sequence, safe.level, safe.message
                    )
                    .unwrap();
                }
            }
            Report::Timings => {
                let timings = &plugin.timings;
                writeln!(
                    &mut section,
                    "Completed: {}; failed: {}; cancelled: {}",
                    timings.completed, timings.failed, timings.cancelled
                )
                .unwrap();
                writeln!(&mut section, "Phase             Last (μs)       Max (μs)\nQueue             {:>9}      {:>9}\nGuest             {:>9}      {:>9}\nHost application  {:>9}      {:>9}", timings.queue_last_us, timings.queue_max_us, timings.execution_last_us, timings.execution_max_us, timings.apply_last_us, timings.apply_max_us).unwrap();
            }
        }
        if output.len() + section.len() + 1 > MAX_REPORT_BYTES {
            let notice = "\nReport truncated at byte limit.\n";
            let remaining = MAX_REPORT_BYTES.saturating_sub(output.len() + notice.len());
            let mut end = remaining.min(section.len());
            while !section.is_char_boundary(end) {
                end -= 1;
            }
            output.push_str(&section[..end]);
            output.push_str(notice);
            break;
        }
        output.push_str(&section);
        output.push('\n');
    }
    if output.is_empty() {
        output.push_str("No configured plugins.\n");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_api::diagnostics::{
        DiagnosticEntry, DiagnosticLevel, PluginEngine, PluginStatus, RequestTimings,
    };
    #[test]
    fn reports_filter_configured_owners_and_render_measured_phases() {
        let fixture = PluginDiagnostics {
            plugin: "fixture".into(),
            generation: 3,
            status: PluginStatus::Ready,
            engine: Some(PluginEngine::Component),
            declared: Default::default(),
            effective: Default::default(),
            queued: 2,
            timings: RequestTimings {
                completed: 7,
                queue_last_us: 11,
                execution_last_us: 22,
                apply_last_us: 33,
                ..Default::default()
            },
            entries: vec![DiagnosticEntry::bounded(
                1,
                DiagnosticLevel::Error,
                "\x1bproblem",
            )],
        };
        let timings = render(
            std::slice::from_ref(&fixture),
            Some("fixture"),
            Report::Timings,
        );
        assert!(
            timings.contains("generation 3")
                && timings.contains("11")
                && timings.contains("22")
                && timings.contains("33")
        );
        assert!(render(
            std::slice::from_ref(&fixture),
            Some("missing"),
            Report::Inspect
        )
        .contains("No configured plugins"));
        let logs = render(&[fixture], None, Report::Logs);
        assert!(logs.contains("problem") && !logs.contains('\x1b'));
    }
}
