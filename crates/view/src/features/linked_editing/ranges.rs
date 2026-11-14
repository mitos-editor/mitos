//! Validate server-linked names and compose their text edits.

#[cfg(test)]
use crate::Document;
use crate::ViewId;
use anyhow::{ensure, Context};
use editor_core::{
    chars::char_is_word, regex::Regex, Assoc, ChangeSet, Range, Rope, Selection, Transaction, Uri,
};
use lsp_client::{lsp, LanguageServerId, OffsetEncoding};
use stdx::rope::RopeExt as _;

pub(super) struct MirroredEdit {
    pub(super) combined: Transaction,
    pub(super) catchup: Transaction,
}

pub(super) struct Session {
    pub(super) view: ViewId,
    pub(super) server: LanguageServerId,
    pub(super) uri: Option<Uri>,
    pub(super) ranges: Vec<Range>,
    pub(super) leader: usize,
    pattern: Option<Regex>,
    extra_word_chars: Vec<char>,
}

impl Session {
    #[cfg(test)]
    fn new(
        doc: &Document,
        view: ViewId,
        server: LanguageServerId,
        cursor: usize,
        response: lsp::LinkedEditingRanges,
        encoding: OffsetEncoding,
    ) -> anyhow::Result<Self> {
        Self::from_text(
            doc.text(),
            doc.uri(),
            view,
            server,
            cursor,
            response,
            encoding,
        )
    }

    pub(super) fn from_text(
        text: &Rope,
        uri: Option<Uri>,
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
            let converted = lsp_client::util::lsp_range_to_range(text, range, encoding)
                .context("Language server returned an invalid linked range")?;
            ensure!(
                lsp_client::util::pos_to_lsp_pos(text, converted.from(), encoding) == range.start
                    && lsp_client::util::pos_to_lsp_pos(text, converted.to(), encoding)
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
        let name = ranges[0].fragment(text.slice(..)).to_string();
        ensure!(
            ranges
                .iter()
                .all(|range| range.fragment(text.slice(..)) == name.as_str()),
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
        let mut extra_word_chars: Vec<_> = name
            .chars()
            .filter(|&c| !char_is_word(c) && !c.is_whitespace() && !c.is_control())
            .collect();
        extra_word_chars.sort_unstable();
        extra_word_chars.dedup();
        let session = Self {
            view,
            server,
            uri,
            ranges,
            leader,
            pattern,
            extra_word_chars,
        };
        ensure!(
            session.valid_word(&name),
            "Linked range contents do not match the word pattern"
        );
        Ok(session)
    }

    fn valid_word(&self, text: &str) -> bool {
        // Empty intermediate names allow deleting a name and typing its replacement.
        text.is_empty()
            || self.pattern.as_ref().map_or_else(
                || {
                    text.chars().all(|c| {
                        char_is_word(c)
                            || matches!(c, '-' | ':' | '.' | '$')
                            || self.extra_word_chars.contains(&c)
                    })
                },
                |pattern| pattern.is_match(text),
            )
    }

    pub(super) fn contains_selection(
        &self,
        view: ViewId,
        selection: &Selection,
        text: &Rope,
    ) -> bool {
        let leader = self.ranges[self.leader];
        view == self.view
            && selection.len() == 1
            && selection.primary().cursor(text.slice(..)) >= leader.from()
            && selection.primary().cursor(text.slice(..)) <= leader.to()
    }

    pub(super) fn mirror(
        &mut self,
        text: &Rope,
        selection: &Selection,
        transaction: &Transaction,
    ) -> Option<MirroredEdit> {
        self.mirror_inner(text, selection, transaction, false)
    }

    pub(super) fn reconcile(
        &mut self,
        text: &Rope,
        selection: &Selection,
        transaction: &Transaction,
    ) -> Option<MirroredEdit> {
        self.mirror_inner(text, selection, transaction, true)
    }

    /// Project cached names through a completion's restoration to its checkpoint.
    pub(super) fn project(&mut self, text: &Rope, changes: &ChangeSet) -> Option<()> {
        if changes.is_empty() {
            return Some(());
        }
        if changes.changes_iter().any(|(from, to, _)| {
            self.ranges.iter().any(|range| {
                (from < range.from() && range.from() < to) || (from < range.to() && range.to() < to)
            })
        }) {
            return None;
        }
        let ranges: Vec<_> = self
            .ranges
            .iter()
            .map(|range| {
                Range::new(
                    changes.map_pos(range.from(), Assoc::Before),
                    changes.map_pos(range.to(), Assoc::After),
                )
            })
            .collect();
        if ranges.windows(2).any(|pair| pair[0].to() > pair[1].from()) {
            return None;
        }
        let name = ranges[0].fragment(text.slice(..));
        if !self.valid_word(&name)
            || ranges
                .iter()
                .any(|range| range.fragment(text.slice(..)) != name)
        {
            return None;
        }
        self.ranges = ranges;
        Some(())
    }

    fn mirror_inner(
        &mut self,
        text: &Rope,
        selection: &Selection,
        transaction: &Transaction,
        allow_outside: bool,
    ) -> Option<MirroredEdit> {
        let leader = self.ranges[self.leader];
        if !transaction.changes_iter().all(|(from, to, _)| {
            (from >= leader.from() && to <= leader.to())
                || (allow_outside
                    && self.ranges.iter().all(|range| {
                        if from == to {
                            from < range.from() || from > range.to()
                        } else {
                            to <= range.from() || from >= range.to()
                        }
                    }))
        }) {
            return None;
        }
        if transaction.changes().is_empty() {
            return Some(MirroredEdit {
                combined: transaction.clone(),
                catchup: transaction.clone(),
            });
        }
        let mut changed = text.clone();
        transaction.apply(&mut changed);
        let start = transaction.changes().map_pos(leader.from(), Assoc::Before);
        let end = transaction.changes().map_pos(leader.to(), Assoc::After);
        let value = changed.slice_chars(start..end).to_string();
        if !self.valid_word(&value) {
            return None;
        }
        let mut mapped = Vec::with_capacity(self.ranges.len());
        for (index, range) in self.ranges.iter().enumerate() {
            let mapped_range = if index == self.leader {
                Range::new(start, end)
            } else {
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
                let mapped = Range::new(from, to);
                if mapped.fragment(changed.slice(..)) != range.fragment(text.slice(..)) {
                    return None;
                }
                mapped
            };
            mapped.push(mapped_range);
        }
        let mirror = Transaction::change(
            &changed,
            mapped
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != self.leader)
                .map(|(_, range)| (range.from(), range.to(), Some(value.as_str().into()))),
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
        let leader_start = (start as i128 + self.leader as i128 * delta) as usize;
        let source_range = selection.primary();
        let selection = if source_range.from() >= start && source_range.to() <= end {
            Selection::single(
                leader_start + source_range.anchor - start,
                leader_start + source_range.head - start,
            )
        } else if source_range.is_single_grapheme(changed.slice(..)) {
            // A block cursor at a name's end selects the following delimiter.
            Selection::point(leader_start + cursor - start)
        } else {
            return None;
        };
        let mut shift = 0i128;
        for (index, (range, current)) in self.ranges.iter_mut().zip(mapped).enumerate() {
            let start = (current.from() as i128 + shift) as usize;
            *range = Range::new(start, start + length);
            if index != self.leader {
                shift += delta;
            }
        }
        let catchup = mirror.with_selection(selection);
        Some(MirroredEdit {
            combined: transaction.clone().compose(catchup.clone()),
            catchup,
        })
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
                .combined
                .apply(&mut text);
            assert_eq!(text, "");
            let insert = Transaction::insert(&text, &Selection::point(0), "x".into())
                .with_selection(Selection::point(1));
            let mirrored = session
                .mirror(&text, &Selection::point(0), &insert)
                .unwrap();
            mirrored.combined.apply(&mut text);
            assert_eq!(text, "xx");
            assert_eq!(
                mirrored.combined.selection(),
                Some(&Selection::point(leader + 1))
            );
            assert_eq!(session.ranges, vec![Range::new(0, 1), Range::new(1, 2)]);
            let cursor = leader + 1;
            let insert = Transaction::insert(&text, &Selection::point(cursor), "y".into())
                .with_selection(Selection::point(cursor + 1));
            let mirrored = session
                .mirror(&text, &Selection::point(cursor), &insert)
                .unwrap();
            mirrored.combined.apply(&mut text);
            assert_eq!(text, "xyxy");
            assert_eq!(
                mirrored.combined.selection(),
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
            .combined
            .apply(&mut text);
        assert_eq!(text, "my--tag my--tag");
        assert!(!session.valid_word("my tag"));
        assert!(!session.valid_word("my<tag"));
    }
}
