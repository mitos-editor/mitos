//! Explicit, view-bound linked editing. Mirroring precedes document publication.

use anyhow::{ensure, Context};
use editor_core::{
    chars::char_is_word, regex::Regex, Assoc, Range, Rope, Selection, Transaction, Uri,
};
use event::{cancelable_future, register_hook, TaskController};
use futures_util::{future::BoxFuture, FutureExt};
use language_config::LanguageServerFeature;
use lsp_client::{lsp, LanguageServerId, OffsetEncoding};
use stdx::rope::RopeExt as _;

use crate::{
    callbacks::EditorCallbackSender,
    document::Mode,
    events::{ConfigDidChange, DocumentFocusLost, LanguageServerExited, SelectionDidChange},
    features::request::SelectionRequest,
    Document, Editor, ViewId,
};

#[derive(Default)]
pub(crate) struct State {
    request: TaskController,
    session: Option<Session>,
}

impl State {
    pub(crate) fn clear(&mut self) {
        self.request.cancel();
        self.session = None;
    }
}

struct Session {
    view: ViewId,
    server: LanguageServerId,
    uri: Option<Uri>,
    ranges: Vec<Range>,
    leader: usize,
    pattern: Option<Regex>,
    extra_word_chars: Vec<char>,
}

struct Target {
    request: SelectionRequest,
    owner: EditorCallbackSender,
    server: LanguageServerId,
    binding: u64,
    mode: Mode,
    cursor: usize,
    encoding: OffsetEncoding,
}

/// One explicit lookup, abandoned when its original editing context changes.
#[must_use]
pub struct LinkedEditingTask {
    target: Target,
    future: BoxFuture<'static, lsp_client::Result<Option<lsp::LinkedEditingRanges>>>,
}

#[must_use]
pub struct LinkedEditingUpdate {
    target: Target,
    result: lsp_client::Result<Option<lsp::LinkedEditingRanges>>,
}

pub fn prepare(editor: &mut Editor, view: ViewId) -> anyhow::Result<LinkedEditingTask> {
    ensure!(editor.config().lsp.enable, "Language servers are disabled");
    ensure!(
        editor.tree.focus() == view,
        "Linked editing requires the focused view"
    );
    let target = editor
        .tree
        .try_get(view)
        .context("Linked editing view is closed")?;
    let (id, binding) = (target.doc, target.binding_revision());
    let mode = editor.mode();
    let owner = editor.callback_sender().clone();
    let doc = editor
        .document_mut(id)
        .context("Linked editing document is closed")?;
    doc.validate_editable()?;
    ensure!(
        doc.selection(view).len() == 1,
        "Linked editing requires one selection"
    );
    ensure!(
        doc.active_snippet().is_none(),
        "Finish the active snippet before linked editing"
    );
    let server = doc
        .language_servers_with_feature(LanguageServerFeature::LinkedEditing)
        .next()
        .context("No configured language server supports linked editing")?;
    let server_id = server.id();
    let encoding = server.offset_encoding();
    let cursor = doc.selection(view).primary().cursor(doc.text().slice(..));
    let position = lsp_client::util::pos_to_lsp_pos(doc.text(), cursor, encoding);
    let future = server
        .linked_editing_ranges(doc.identifier(), position)
        .context("Language server rejected linked editing")?
        .boxed();
    doc.linked_editing.clear();
    let handle = doc.linked_editing.request.restart();
    let request = SelectionRequest::new(doc, view, handle, vec![server_id]);
    Ok(LinkedEditingTask {
        target: Target {
            request,
            owner,
            server: server_id,
            binding,
            mode,
            cursor,
            encoding,
        },
        future,
    })
}

impl LinkedEditingTask {
    pub async fn run(self) -> Option<LinkedEditingUpdate> {
        let result = cancelable_future(self.future, self.target.request.cancel_handle()).await?;
        Some(LinkedEditingUpdate {
            target: self.target,
            result,
        })
    }
}

impl LinkedEditingUpdate {
    pub fn apply(self, editor: &mut Editor) -> anyhow::Result<bool> {
        let target = self.target;
        let Some((view, doc)) = target.request.get(editor) else {
            return Ok(false);
        };
        if !target.owner.same_destination(editor.callback_sender())
            || !editor.config().lsp.enable
            || editor.tree.focus() != view.id
            || editor.mode() != target.mode
            || view.binding_revision() != target.binding
            || doc.active_snippet().is_some()
            || !doc
                .language_servers_with_feature(LanguageServerFeature::LinkedEditing)
                .next()
                .is_some_and(|server| server.id() == target.server)
        {
            return Ok(false);
        }
        let response = self
            .result?
            .context("No linked editing ranges at the cursor")?;
        doc.validate_editable()?;
        let view = view.id;
        let id = doc.id();
        let session = Session::new(
            doc,
            view,
            target.server,
            target.cursor,
            response,
            target.encoding,
        )?;
        editor.set_mode(Mode::Insert);
        let doc = editor.document_mut(id).unwrap();
        doc.commit_changes();
        doc.set_selection(view, Selection::point(target.cursor));
        doc.linked_editing.session = Some(session);
        editor.redraw().request();
        Ok(true)
    }
}

impl Session {
    fn new(
        doc: &Document,
        view: ViewId,
        server: LanguageServerId,
        cursor: usize,
        response: lsp::LinkedEditingRanges,
        encoding: OffsetEncoding,
    ) -> anyhow::Result<Self> {
        ensure!(
            response.ranges.len() >= 2,
            "Language server returned fewer than two linked ranges"
        );
        let mut ranges = Vec::with_capacity(response.ranges.len());
        for range in response.ranges {
            ensure!(
                range.start < range.end,
                "Language server returned an empty or reversed linked range"
            );
            let converted = lsp_client::util::lsp_range_to_range(doc.text(), range, encoding)
                .context("Language server returned an invalid linked range")?;
            ensure!(
                lsp_client::util::pos_to_lsp_pos(doc.text(), converted.from(), encoding)
                    == range.start
                    && lsp_client::util::pos_to_lsp_pos(doc.text(), converted.to(), encoding)
                        == range.end,
                "Language server returned an invalid linked range position"
            );
            ranges.push(converted);
        }
        ranges.sort_by_key(Range::from);
        ensure!(
            ranges.windows(2).all(|pair| pair[0].to() <= pair[1].from()),
            "Language server returned overlapping linked ranges"
        );
        let text = ranges[0].fragment(doc.text().slice(..)).to_string();
        ensure!(
            ranges
                .iter()
                .all(|range| range.fragment(doc.text().slice(..)) == text.as_str()),
            "Language server returned linked ranges with different contents"
        );
        let leader = ranges
            .iter()
            .position(|range| range.from() <= cursor && cursor < range.to())
            .or_else(|| ranges.iter().position(|range| cursor == range.to()))
            .context("Language server returned linked ranges outside the cursor")?;
        let pattern = response
            .word_pattern
            .map(|pattern| Regex::new(&format!(r"\A(?:{pattern})\z")))
            .transpose()
            .context("Language server returned an unsupported linked editing word pattern")?;
        let mut extra_word_chars: Vec<_> = text
            .chars()
            .filter(|&c| !char_is_word(c) && !c.is_whitespace() && !c.is_control())
            .collect();
        extra_word_chars.sort_unstable();
        extra_word_chars.dedup();
        let session = Self {
            view,
            server,
            uri: doc.uri(),
            ranges,
            leader,
            pattern,
            extra_word_chars,
        };
        ensure!(
            session.valid_word(&text),
            "Linked range contents do not match the word pattern"
        );
        Ok(session)
    }

    fn valid_word(&self, text: &str) -> bool {
        // Empty intermediate names allow deleting a name and typing its replacement.
        text.is_empty()
            || self.pattern.as_ref().map_or_else(
                || {
                    text.chars()
                        .all(|c| char_is_word(c) || self.extra_word_chars.contains(&c))
                },
                |pattern| pattern.is_match(text),
            )
    }

    fn contains_selection(&self, view: ViewId, selection: &Selection, text: &Rope) -> bool {
        let leader = self.ranges[self.leader];
        view == self.view
            && selection.len() == 1
            && selection.primary().cursor(text.slice(..)) >= leader.from()
            && selection.primary().cursor(text.slice(..)) <= leader.to()
    }

    fn mirror(
        &mut self,
        text: &Rope,
        selection: &Selection,
        transaction: &Transaction,
    ) -> Option<Transaction> {
        let leader = self.ranges[self.leader];
        if !transaction
            .changes_iter()
            .all(|(from, to, _)| from >= leader.from() && to <= leader.to())
        {
            return None;
        }
        if transaction.changes().is_empty() {
            return Some(transaction.clone());
        }
        let mut changed = text.clone();
        transaction.apply(&mut changed);
        let start = transaction.changes().map_pos(leader.from(), Assoc::Before);
        let end = transaction.changes().map_pos(leader.to(), Assoc::After);
        let value = changed.slice_chars(start..end).to_string();
        if !self.valid_word(&value) {
            return None;
        }
        let mirror = Transaction::change(
            &changed,
            self.ranges
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != self.leader)
                .map(|(index, range)| {
                    let (from, to) = if range.is_empty() {
                        // Deleting adjacent names may collapse them onto the same
                        // position. Reinsert each name on its original side of the leader.
                        let assoc = if index < self.leader {
                            Assoc::Before
                        } else {
                            Assoc::After
                        };
                        let at = transaction.changes().map_pos(range.from(), assoc);
                        (at, at)
                    } else {
                        (
                            transaction.changes().map_pos(range.from(), Assoc::After),
                            transaction.changes().map_pos(range.to(), Assoc::Before),
                        )
                    };
                    (from, to, Some(value.as_str().into()))
                }),
        );
        let selection = transaction
            .selection()
            .cloned()
            .unwrap_or_else(|| selection.clone().map(transaction.changes()));
        let cursor = selection.primary().cursor(changed.slice(..));
        if selection.len() != 1 || cursor < start || cursor > end {
            return None;
        }
        let length = value.chars().count();
        let delta = length as i128 - (leader.to() - leader.from()) as i128;
        // Mirrored replacements may touch the leader at either boundary. Keep
        // the caret relative to the edited name rather than mapping it across
        // a following replacement at the same position.
        let leader_start = (leader.from() as i128 + self.leader as i128 * delta) as usize;
        let selection = Selection::point(leader_start + cursor - start);
        for (index, range) in self.ranges.iter_mut().enumerate() {
            let start = (range.from() as i128 + index as i128 * delta) as usize;
            *range = Range::new(start, start + length);
        }
        Some(
            transaction
                .clone()
                .compose(mirror)
                .with_selection(selection),
        )
    }
}

/// Called before mutation so synchronization and observers see one combined change.
pub(crate) fn expand(
    doc: &mut Document,
    transaction: &Transaction,
    origin: Option<ViewId>,
) -> Option<Transaction> {
    doc.linked_editing.request.cancel();
    let mut session = doc.linked_editing.session.take()?;
    let view = origin?;
    if session.uri != doc.uri()
        || doc.active_snippet().is_some()
        || !session.contains_selection(view, doc.selection(view), doc.text())
        || !doc
            .language_servers_with_feature(LanguageServerFeature::LinkedEditing)
            .next()
            .is_some_and(|server| server.id() == session.server)
    {
        return None;
    }
    let expanded = session.mirror(doc.text(), doc.selection(view), transaction)?;
    doc.linked_editing.session = Some(session);
    Some(expanded)
}

pub(crate) fn register_hooks(events: &event::EventHub) {
    register_hook!(events => move |event: &mut SelectionDidChange<'_>| {
        event.doc.linked_editing.request.cancel();
        if event.doc.linked_editing.session.as_ref().is_some_and(|session|
            !event.doc.try_selection(event.view).is_some_and(|selection|
                session.contains_selection(event.view, selection, event.doc.text()))) {
            event.doc.linked_editing.session = None;
        }
        Ok(())
    });
    register_hook!(events => move |event: &mut DocumentFocusLost<'_>| {
        if let Some(doc) = event.editor.document_mut(event.doc) { doc.linked_editing.clear(); }
        Ok(())
    });
    register_hook!(events => move |event: &mut ConfigDidChange<'_>| {
        cancel(event.editor);
        Ok(())
    });
    register_hook!(events => move |event: &mut LanguageServerExited<'_>| {
        for doc in event.editor.documents_mut() {
            if doc.supports_language_server(event.server_id) { doc.linked_editing.clear(); }
        }
        Ok(())
    });
}

pub(crate) fn cancel(editor: &mut Editor) {
    for doc in editor.documents_mut() {
        doc.linked_editing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(ranges: &[(u32, u32)]) -> lsp::LinkedEditingRanges {
        lsp::LinkedEditingRanges {
            ranges: ranges
                .iter()
                .map(|&(from, to)| {
                    lsp::Range::new(lsp::Position::new(0, from), lsp::Position::new(0, to))
                })
                .collect(),
            word_pattern: None,
        }
    }

    #[tokio::test]
    async fn adjacent_names_can_be_deleted_and_retyped_from_either_occurrence() {
        let (editor, _dir) = crate::editor::tests::editor("#[|a]#a");
        let (view, doc) = current_ref!(editor);
        for leader in [0, 1] {
            let mut session = Session::new(
                doc,
                view.id,
                Default::default(),
                leader,
                response(&[(0, 1), (1, 2)]),
                OffsetEncoding::Utf16,
            )
            .unwrap();
            assert_eq!(session.leader, leader);
            let mut text = doc.text().clone();
            let delete = Transaction::change(&text, [(leader, leader + 1, None)].into_iter())
                .with_selection(Selection::point(leader));
            session
                .mirror(&text, &Selection::point(leader), &delete)
                .unwrap()
                .apply(&mut text);
            assert_eq!(text, "");
            let insert = Transaction::insert(&text, &Selection::point(0), "x".into())
                .with_selection(Selection::point(1));
            let mirrored = session
                .mirror(&text, &Selection::point(0), &insert)
                .unwrap();
            mirrored.apply(&mut text);
            assert_eq!(text, "xx");
            assert_eq!(mirrored.selection(), Some(&Selection::point(leader + 1)));
            assert_eq!(session.ranges, vec![Range::new(0, 1), Range::new(1, 2)]);
            let cursor = leader + 1;
            let insert = Transaction::insert(&text, &Selection::point(cursor), "y".into())
                .with_selection(Selection::point(cursor + 1));
            let mirrored = session
                .mirror(&text, &Selection::point(cursor), &insert)
                .unwrap();
            mirrored.apply(&mut text);
            assert_eq!(text, "xyxy");
            assert_eq!(
                mirrored.selection(),
                Some(&Selection::point(if leader == 0 { 2 } else { 4 }))
            );
        }
    }

    #[tokio::test]
    async fn missing_word_patterns_preserve_punctuation_in_server_selected_names() {
        let (editor, _dir) = crate::editor::tests::editor("#[|m]#y-tag my-tag");
        let (view, doc) = current_ref!(editor);
        let mut session = Session::new(
            doc,
            view.id,
            Default::default(),
            2,
            response(&[(0, 6), (7, 13)]),
            OffsetEncoding::Utf16,
        )
        .unwrap();
        let mut text = doc.text().clone();
        let insert = Transaction::insert(&text, &Selection::point(2), "-".into());
        session
            .mirror(&text, &Selection::point(2), &insert)
            .unwrap()
            .apply(&mut text);
        assert_eq!(text, "my--tag my--tag");
        assert!(!session.valid_word("my tag"));
        assert!(!session.valid_word("my<tag"));
    }
}
