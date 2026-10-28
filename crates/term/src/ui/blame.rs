//! Commit details and copy actions for the primary cursor's blame.

use theme::Theme;
use tui::{
    buffer::Buffer,
    text::{Line, Span, Text},
    widgets::Widget,
};
use ui_core::keymap::{KeymapState, Lookup};
use vcs::LineBlame;
use view::{features::blame::BlameDetailsUpdate, graphics::Rect, Editor};

use crate::{
    compositor::{Component, Compositor, Context, Event, EventResult},
    keymap::component::{action_hint, BlameAction, PopupAction},
    ui::{panel, text, Popup},
};

pub struct BlameDetails {
    keys: KeymapState,
    commit_id: String,
    author: String,
    date: String,
    title: String,
    body: Option<String>,
    message: String,
    #[cfg(feature = "git")]
    source: Option<(std::path::PathBuf, std::path::PathBuf)>,
}

impl BlameDetails {
    pub const ID: &'static str = "blame-details";

    fn new(blame: LineBlame) -> Self {
        let title = blame.commit_title.unwrap_or_default();
        let body = blame.commit_body.filter(|body| !body.is_empty());
        let mut message = title.clone();
        if let Some(body) = &body {
            message.push_str("\n\n");
            message.push_str(body);
        }
        let author = match (blame.author_name, blame.author_email) {
            (Some(name), Some(email)) => format!("{name} <{email}>"),
            (Some(name), None) => name,
            (None, Some(email)) => email,
            (None, None) => "Unknown author".into(),
        };
        Self {
            keys: KeymapState::default(),
            commit_id: blame.commit_id,
            author,
            date: blame.commit_date.unwrap_or_else(|| "Unknown date".into()),
            title,
            body,
            message,
            #[cfg(feature = "git")]
            source: None,
        }
    }

    fn text(&self, theme: Option<&Theme>) -> Text<'_> {
        let style = |scope| {
            theme
                .map(|theme| theme.get("ui.text").patch(theme.get(scope)))
                .unwrap_or_default()
        };
        let label = style("keyword");
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Commit: ", label),
                Span::styled(&self.commit_id, style("constant")),
            ]),
            Line::from(vec![
                Span::styled("Author: ", label),
                Span::styled(&self.author, style("string")),
            ]),
            Line::from(vec![
                Span::styled("Date: ", label),
                Span::styled(&self.date, style("constant.numeric")),
            ]),
            Line::default(),
        ];
        lines.extend(
            self.title
                .lines()
                .map(|line| Line::from(Span::styled(line, style("markup.heading.1")))),
        );
        // Commit messages are literal text, including Markdown and code fences.
        if let Some(body) = &self.body {
            lines.push(Line::default());
            lines.extend(
                body.lines()
                    .map(|line| Line::from(Span::styled(line, style("ui.text")))),
            );
        }
        Text::from(lines)
    }
}

pub fn show(
    editor: &mut Editor,
    compositor: &mut Compositor,
    update: BlameDetailsUpdate,
    config: &crate::config::Config,
) {
    let Some(result) = update.resolve(editor) else {
        return;
    };
    match result {
        Ok(blame) => {
            let details = BlameDetails::new(blame);
            #[cfg(feature = "git")]
            let details = {
                let mut details = details;
                let (_, doc) = view::current_ref!(editor);
                details.source = doc
                    .path()
                    .map(|path| (path.to_owned(), doc.workspace_root().to_owned()));
                details
            };
            let popup = Popup::new(BlameDetails::ID, details).auto_close(true);
            compositor.replace_or_push(BlameDetails::ID, popup);
            let keys = &config.component_keys;
            editor.set_status(format!(
                "Blame: {} copies ID; {} copies message; {} copies link; {} opens link; {} closes",
                action_hint(&keys.blame, &BlameAction::CopyCommitId),
                action_hint(&keys.blame, &BlameAction::CopyMessage),
                action_hint(&keys.blame, &BlameAction::CopyCommitLink),
                action_hint(&keys.blame, &BlameAction::OpenCommitLink),
                action_hint(&keys.popup, &PopupAction::Cancel),
            ));
        }
        Err(message) => editor.set_error(|| message),
    }
}

impl Component for BlameDetails {
    fn has_pending_keys(&self) -> bool {
        self.keys.is_pending()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };
        match self
            .keys
            .lookup(&cx.config.current.component_keys.blame, *key)
        {
            Lookup::Pending => EventResult::Consumed(None),
            Lookup::Unbound => EventResult::Ignored(None),
            Lookup::Actions(actions) => {
                for action in actions {
                    if matches!(
                        action,
                        BlameAction::CopyCommitLink | BlameAction::OpenCommitLink
                    ) {
                        #[cfg(feature = "git")]
                        if let Some((path, workspace)) = &self.source {
                            crate::commands::request_commit_permalink(
                                cx.editor,
                                cx.jobs,
                                path.clone(),
                                workspace.clone(),
                                self.commit_id.clone(),
                                *action == BlameAction::OpenCommitLink,
                            );
                        }
                        continue;
                    }
                    let (text, label) = match action {
                        BlameAction::CopyCommitId => (&self.commit_id, "commit ID"),
                        BlameAction::CopyMessage => (&self.message, "commit message"),
                        BlameAction::CopyCommitLink | BlameAction::OpenCommitLink => unreachable!(),
                    };
                    match cx.editor.registers.write('+', vec![text.clone()]) {
                        Ok(()) => cx.editor.set_status(format!("Copied {label}")),
                        Err(err) => cx.editor.set_error(|| err.to_string()),
                    }
                }
                EventResult::Consumed(None)
            }
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        text::paragraph(self.text(Some(&cx.editor.theme)))
            .scroll((cx.scroll.unwrap_or_default() as u16, 0))
            .render(panel::content_area(area), surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let contents = text::paragraph(self.text(None));
        let max_text_width = viewport.0.saturating_sub(panel::HORIZONTAL_INSET).min(120);
        let (width, height) = text::required_size(&contents, max_text_width);
        Some((
            width + panel::HORIZONTAL_INSET,
            height + panel::VERTICAL_INSET,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_hints_follow_remapped_keys_and_sequences() {
        let source = "[keys.blame]\ny = 'unbound'\nY = 'unbound'\n[keys.blame.g]\nc = 'copy_commit_id'\nm = 'copy_message'".to_owned();
        let config = crate::config::Config::load(Ok(&source), Ok(String::new())).unwrap();
        assert_eq!(
            action_hint(&config.component_keys.blame, &BlameAction::CopyCommitId),
            "gc"
        );
        assert_eq!(
            action_hint(&config.component_keys.blame, &BlameAction::CopyMessage),
            "gm"
        );
    }
}
