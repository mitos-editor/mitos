//! Commit details and copy actions for the primary cursor's blame.

use tui::buffer::Buffer;
use ui_core::keymap::{Binding, Keymap, KeymapState, Lookup};
use vcs::LineBlame;
use view::{features::blame::BlameDetailsUpdate, graphics::Rect, Editor};

use crate::{
    compositor::{Component, Compositor, Context, Event, EventResult},
    keymap::component::{BlameAction, PopupAction},
    ui::{Markdown, Popup},
};

pub struct BlameDetails {
    keys: KeymapState,
    contents: Markdown,
    commit_id: String,
    message: String,
}

impl BlameDetails {
    pub const ID: &'static str = "blame-details";

    fn new(blame: LineBlame, editor: &Editor) -> Self {
        let mut message = blame.commit_title.unwrap_or_default();
        if let Some(body) = blame.commit_body.filter(|body| !body.is_empty()) {
            message.push_str("\n\n");
            message.push_str(&body);
        }
        let author = match (blame.author_name, blame.author_email) {
            (Some(name), Some(email)) => format!("{name} <{email}>"),
            (Some(name), None) => name,
            (None, Some(email)) => email,
            (None, None) => "Unknown author".into(),
        };
        let text = format!(
            "Commit: {}\nAuthor: {author}\nDate: {}\n\n{message}",
            blame.commit_id,
            blame.commit_date.as_deref().unwrap_or("Unknown date"),
        );
        // Commit messages are literal text, including Markdown and code fences.
        let contents = text
            .lines()
            .map(|line| format!("    {line}\n"))
            .collect::<String>();
        Self {
            keys: KeymapState::default(),
            contents: Markdown::new(contents, editor.syn_loader.clone()),
            commit_id: blame.commit_id,
            message,
        }
    }
}

fn action_hint<A: PartialEq>(keys: &Keymap<A>, action: &A) -> String {
    keys.0
        .iter()
        .filter_map(|(key, binding)| {
            let suffix = match binding {
                Binding::Actions(actions) if actions.as_slice() == std::slice::from_ref(action) => {
                    String::new()
                }
                Binding::Keys(keys) => {
                    let hint = action_hint(keys, action);
                    if hint == "unbound" {
                        return None;
                    }
                    hint
                }
                _ => return None,
            };
            Some(format!("{}{suffix}", key.key_sequence_format()))
        })
        .min_by_key(|hint| (hint.len(), hint.clone()))
        .unwrap_or_else(|| "unbound".into())
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
            let popup =
                Popup::new(BlameDetails::ID, BlameDetails::new(blame, editor)).auto_close(true);
            compositor.replace_or_push(BlameDetails::ID, popup);
            let keys = &config.component_keys;
            editor.set_status(format!(
                "Blame: {} copies commit ID; {} copies message; {} closes",
                action_hint(&keys.blame, &BlameAction::CopyCommitId),
                action_hint(&keys.blame, &BlameAction::CopyMessage),
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
                    let (text, label) = match action {
                        BlameAction::CopyCommitId => (&self.commit_id, "commit ID"),
                        BlameAction::CopyMessage => (&self.message, "commit message"),
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
        self.contents.render(area, surface, cx);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.contents.required_size(viewport)
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
