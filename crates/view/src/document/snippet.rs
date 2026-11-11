//! One document's active snippet and pending placeholder replacement.

mod variables;

use super::Document;
use crate::{editor::SnippetNavigation, ViewId};
use editor_core::{movement::Direction, ChangeSet, Transaction};
use snippets::{ActiveSnippet, RenderedSnippet};

#[derive(Clone)]
pub(super) struct SnippetSession {
    pub(super) active: ActiveSnippet,
    view: ViewId,
    replace_placeholder: bool,
}

impl Document {
    /// Read snippet tabstops for highlighting without exposing mutable session state.
    pub fn active_snippet(&self) -> Option<&ActiveSnippet> {
        self.snippet.as_ref().map(|session| &session.active)
    }

    pub(crate) fn clear_snippet(&mut self) {
        self.snippet = None;
    }

    pub(crate) fn install_snippet(&mut self, view: ViewId, rendered: RenderedSnippet) {
        let active = match self.snippet.take().filter(|session| session.view == view) {
            Some(session) => session.active.insert_subsnippet(rendered),
            None => ActiveSnippet::new(rendered),
        };
        self.snippet = active.map(|active| SnippetSession {
            active,
            view,
            replace_placeholder: true,
        });
    }

    pub(crate) fn move_snippet_tabstop(
        &mut self,
        view: ViewId,
        direction: Direction,
    ) -> SnippetNavigation {
        let Some(mut session) = self.snippet.take() else {
            return SnippetNavigation::Inactive;
        };
        if session.view != view {
            self.snippet = Some(session);
            return SnippetNavigation::Inactive;
        }
        let current = self.snippet_selection(view);
        let target = match direction {
            Direction::Forward => Some(session.active.next_tabstop(&current)),
            Direction::Backward => session
                .active
                .prev_tabstop(&current)
                .map(|selection| (selection, false)),
        };
        let Some((selection, last)) = target else {
            self.snippet = Some(session);
            return SnippetNavigation::AtStart;
        };
        if !last {
            self.snippet = Some(session);
        }
        self.set_selection(view, selection);
        if let Some(session) = &mut self.snippet {
            session.replace_placeholder = true;
        }
        if last {
            SnippetNavigation::Finished
        } else {
            SnippetNavigation::Moved
        }
    }

    pub(crate) fn snippet_choice_transaction(
        &self,
        view: ViewId,
        direction: Direction,
    ) -> Option<Transaction> {
        let session = self
            .snippet
            .as_ref()
            .filter(|session| session.view == view)?;
        session
            .active
            .cycle_choice(self.text(), &self.snippet_selection(view), direction)
    }

    pub(crate) fn snippet_placeholder_active(&self, view: ViewId) -> bool {
        self.snippet
            .as_ref()
            .is_some_and(|session| session.view == view && session.replace_placeholder)
    }

    pub(crate) fn pending_snippet_placeholder(&self, view: ViewId) -> Option<Transaction> {
        let session = self.snippet.as_ref()?;
        (session.view == view && session.replace_placeholder)
            .then(|| session.active.delete_placeholder(self.text()))
    }

    pub(crate) fn snippet_input_applied(&mut self, view: ViewId) {
        if let Some(session) = &mut self.snippet
            && session.view == view
        {
            session.replace_placeholder = false;
        }
    }

    pub(super) fn validate_snippet_selection(&mut self, view: ViewId, changed: bool) {
        let selection = self.selection(view).clone();
        validate(
            &mut self.snippet,
            view,
            selection,
            self.buffer.text(),
            changed,
        );
    }

    // Insert cursors are stored as one-grapheme selections. At a snippet's
    // end that grapheme (including a CRLF pair) belongs to the following text.
    fn snippet_selection(&self, view: ViewId) -> editor_core::Selection {
        snippet_selection(self.selection(view).clone(), self.buffer.text())
    }

    pub(super) fn map_snippet(&mut self, changes: &ChangeSet) {
        map(&mut self.snippet, changes);
    }
}

pub(super) fn validate(
    snippet: &mut Option<SnippetSession>,
    view: ViewId,
    selection: editor_core::Selection,
    text: &editor_core::Rope,
    changed: bool,
) {
    let selection = snippet_selection(selection, text);
    if let Some(session) = snippet
        && session.view == view
    {
        if changed {
            session.replace_placeholder = false;
        }
        if !session.active.is_valid(&selection) {
            *snippet = None;
        }
    }
}
pub(super) fn map(snippet: &mut Option<SnippetSession>, changes: &ChangeSet) {
    if snippet
        .as_mut()
        .is_some_and(|session| session.active.map(changes))
    {
        *snippet = None;
    }
}

fn snippet_selection(
    selection: editor_core::Selection,
    text: &editor_core::Rope,
) -> editor_core::Selection {
    selection.transform(|range| {
        if range.is_single_grapheme(text.slice(..)) {
            editor_core::Range::point(range.cursor(text.slice(..)))
        } else {
            range
        }
    })
}
