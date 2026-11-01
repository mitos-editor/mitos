use editor_core::regex::Regex;
use editor_core::{
    ropey::extra::esoterica::disconnect_slice, ChangeSet, ChangeSetBuilder, Operation, Rope,
    Transaction,
};
use std::num::NonZeroUsize;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// One step through the text history. Selection restoration belongs to the caller.
#[derive(Debug, Clone)]
pub struct HistoryStep {
    pub revision: usize,
    pub transaction: Transaction,
    pub direction: HistoryDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryDirection {
    Undo,
    Redo,
}

/// Text history navigation, independent of any editor's cursor state.
#[derive(Debug, Clone, Copy)]
pub enum HistoryAction {
    Undo,
    Redo,
    Earlier(UndoKind),
    Later(UndoKind),
}

/// Stores text history independently of cursors and views. Callers can associate
/// optional selection snapshots with revision identifiers returned by commits and
/// [`HistoryStep`]s returned by undo, redo, and chronological navigation.
///
/// Currently the history is represented as a vector of revisions. The vector
/// always has at least one element: the empty root revision. Each revision
/// with the exception of the root has a parent revision, a [Transaction]
/// that can be applied to its parent to transition from the parent to itself,
/// and an inversion of that transaction to transition from the parent to its
/// latest child.
///
/// When using `u` to undo a change, an inverse of the stored transaction will
/// be applied which will transition the buffer to the parent state.
///
/// Each revision with the exception of the last in the vector also has a
/// last child revision. When using `U` to redo a change, the last child transaction
/// will be applied to the current state of the buffer.
///
/// The current revision is the one currently displayed in the buffer.
///
/// Committing a new revision to the history will update the last child of the
/// current revision, and push a new revision to the end of the vector.
///
/// Revisions are committed with a timestamp. :earlier and :later can be used
/// to jump to the closest revision to a moment in time relative to the timestamp
/// of the current revision plus (:later) or minus (:earlier) the duration
/// given to the command. If a single integer is given, the editor will instead
/// jump the given number of revisions in the vector.
///
/// Limitations:
///  * The vector of history revisions is currently unbounded. This might
///    cause the memory consumption to grow significantly large during long
///    editing sessions.
///  * Because delete transactions currently don't store the text that they
///    delete, we also store an inversion of the transaction.
///
/// Using time to navigate the history: <https://github.com/helix-editor/helix/pull/194>
#[derive(Debug)]
pub struct History {
    revisions: Vec<Revision>,
    current: usize,
}

/// A single point in history. See [History] for more information.
#[derive(Debug, Clone)]
struct Revision {
    parent: usize,
    last_child: Option<NonZeroUsize>,
    transaction: Transaction,
    // We need an inversion for undos because delete transactions don't store
    // the deleted text.
    inversion: Inversion,
    timestamp: Instant,
}

/// Keep ordinary edits compact. Large inverse inserts can share immutable rope
/// storage until replay needs a contiguous transaction for document observers.
#[derive(Debug, Clone)]
enum Inversion {
    Copied(Transaction),
    Shared {
        operations: Vec<InverseOperation>,
        len_before: usize,
        len_after: usize,
    },
}

#[derive(Debug, Clone)]
enum InverseOperation {
    Retain(usize),
    Delete(usize),
    Insert(Rope),
}

impl Inversion {
    fn new(changes: &ChangeSet, original: &Rope) -> Self {
        const MIN_SHARED_BYTES: usize = 64 * 1024;
        assert_eq!(original.len_chars(), changes.len_before());
        let deleted_chars: usize = changes
            .changes()
            .iter()
            .filter_map(|op| match op {
                Operation::Delete(n) => Some(*n),
                _ => None,
            })
            .sum();
        // UTF-8 characters occupy at most four bytes. Avoid even collecting
        // ranges for the small edits that dominate normal typing.
        if deleted_chars < MIN_SHARED_BYTES / 4 {
            return Self::Copied(Transaction::from(changes.invert(original)));
        }
        let mut pos = 0;
        let mut deleted = Vec::new();
        for operation in changes.changes() {
            match operation {
                Operation::Retain(n) => pos += n,
                Operation::Delete(n) => {
                    deleted.push(
                        original.slice(
                            original.char_to_byte_idx(pos)..original.char_to_byte_idx(pos + n),
                        ),
                    );
                    pos += n;
                }
                Operation::Insert(_) => {}
            }
        }
        let deleted_bytes: usize = deleted.iter().map(|text| text.len()).sum();
        // Disconnected slices can retain their entire backing tree. Share only
        // when that tree is at most twice the size of the deleted text in total.
        if deleted_bytes < MIN_SHARED_BYTES || deleted_bytes < original.len().div_ceil(2) {
            return Self::Copied(Transaction::from(changes.invert(original)));
        }
        let mut deleted = deleted.into_iter();
        let mut operations = Vec::with_capacity(changes.changes().len());
        for operation in changes.changes() {
            match operation {
                Operation::Retain(n) => operations.push(InverseOperation::Retain(*n)),
                Operation::Insert(text) => {
                    operations.push(InverseOperation::Delete(text.chars().count()))
                }
                Operation::Delete(_) => {
                    let text = disconnect_slice(deleted.next().unwrap())
                        .expect("history slices always originate from a rope");
                    let insert = InverseOperation::Insert(text);
                    // Match ChangeSetBuilder's canonical insert-before-delete order.
                    if let Some(last @ InverseOperation::Delete(_)) = operations.last_mut() {
                        let delete = std::mem::replace(last, insert);
                        operations.push(delete);
                    } else {
                        operations.push(insert);
                    }
                }
            }
        }
        Self::Shared {
            operations,
            len_before: changes.len_after(),
            len_after: changes.len_before(),
        }
    }

    fn transaction(&self) -> Transaction {
        match self {
            Self::Copied(transaction) => transaction.clone(),
            Self::Shared { operations, .. } => {
                let mut changes = ChangeSetBuilder::with_capacity(operations.len());
                for operation in operations {
                    match operation {
                        InverseOperation::Retain(n) => changes.retain(*n),
                        InverseOperation::Delete(n) => changes.delete(*n),
                        InverseOperation::Insert(text) => {
                            let mut fragment = String::with_capacity(text.len());
                            for chunk in text.chunks() {
                                fragment.push_str(chunk);
                            }
                            changes.insert(fragment.into());
                        }
                    }
                }
                Transaction::from(changes.finish())
            }
        }
    }

    fn matches(&self, changes: &ChangeSet) -> bool {
        match self {
            Self::Copied(transaction) => transaction.changes() == changes,
            Self::Shared {
                operations,
                len_before,
                len_after,
            } => {
                *len_before == changes.len_before()
                    && *len_after == changes.len_after()
                    && operations.len() == changes.changes().len()
                    && operations
                        .iter()
                        .zip(changes.changes())
                        .all(|(stored, supplied)| match (stored, supplied) {
                            (InverseOperation::Retain(a), Operation::Retain(b))
                            | (InverseOperation::Delete(a), Operation::Delete(b)) => a == b,
                            (InverseOperation::Insert(a), Operation::Insert(b)) => {
                                a.slice(..) == b.as_str()
                            }
                            _ => false,
                        })
            }
        }
    }
}

impl Default for History {
    fn default() -> Self {
        // Add a dummy root revision with empty transaction
        Self {
            revisions: vec![Revision {
                parent: 0,
                last_child: None,
                transaction: Transaction::from(ChangeSet::new("".into())),
                inversion: Inversion::Copied(Transaction::from(ChangeSet::new("".into()))),
                timestamp: Instant::now(),
            }],
            current: 0,
        }
    }
}

impl History {
    /// Commit text changes and return their stable revision identifier.
    pub fn commit_revision(&mut self, changes: &ChangeSet, original: &Rope) -> usize {
        self.commit_revision_at_timestamp(changes, original, Instant::now())
    }

    pub fn commit_revision_at_timestamp(
        &mut self,
        changes: &ChangeSet,
        original: &Rope,
        timestamp: Instant,
    ) -> usize {
        let transaction = Transaction::from(changes.clone());
        let inversion = Inversion::new(changes, original);
        let new_current = self.revisions.len();
        self.revisions[self.current].last_child = NonZeroUsize::new(new_current);
        self.revisions.push(Revision {
            parent: self.current,
            last_child: None,
            transaction,
            inversion,
            timestamp,
        });
        self.current = new_current;
        new_current
    }

    #[inline]
    pub fn current_revision(&self) -> usize {
        self.current
    }

    #[inline]
    pub const fn at_root(&self) -> bool {
        self.current == 0
    }

    /// Returns the changes since the given revision composed into a transaction.
    /// Returns None if there are no changes between the current and given revisions.
    pub fn changes_since(&self, revision: usize) -> Option<Transaction> {
        let lca = self.lowest_common_ancestor(revision, self.current);
        let up = self.path_up(revision, lca);
        let down = self.path_up(self.current, lca);
        let up_txns = up
            .iter()
            .rev()
            .map(|&n| self.revisions[n].inversion.transaction());
        let down_txns = down.iter().map(|&n| self.revisions[n].transaction.clone());

        down_txns.chain(up_txns).reduce(|acc, tx| tx.compose(acc))
    }

    /// Undo text independently of any editor's cursor state.
    pub fn undo(&mut self) -> Option<HistoryStep> {
        if self.at_root() {
            return None;
        }
        let revision = self.current;
        self.current = self.revisions[revision].parent;
        Some(self.step(revision, HistoryDirection::Undo))
    }

    pub fn redo(&mut self) -> Option<HistoryStep> {
        let revision = self.revisions[self.current].last_child?.get();
        self.current = revision;
        Some(self.step(revision, HistoryDirection::Redo))
    }

    /// Text changes at the current revision, without editor selection metadata.
    pub fn current_transaction(&self) -> Option<&Transaction> {
        (self.current != 0).then(|| &self.revisions[self.current].transaction)
    }

    fn step(&self, revision: usize, direction: HistoryDirection) -> HistoryStep {
        let entry = &self.revisions[revision];
        HistoryStep {
            revision,
            transaction: match direction {
                HistoryDirection::Undo => entry.inversion.transaction(),
                HistoryDirection::Redo => entry.transaction.clone(),
            },
            direction,
        }
    }

    fn lowest_common_ancestor(&self, mut a: usize, mut b: usize) -> usize {
        use std::collections::HashSet;
        let mut a_path_set = HashSet::new();
        let mut b_path_set = HashSet::new();
        loop {
            a_path_set.insert(a);
            b_path_set.insert(b);
            if a_path_set.contains(&b) {
                return b;
            }
            if b_path_set.contains(&a) {
                return a;
            }
            a = self.revisions[a].parent; // Relies on the parent of 0 being 0.
            b = self.revisions[b].parent; // Same as above.
        }
    }

    /// List of nodes on the way from `n` to 'a`. Doesn't include `a`.
    /// Includes `n` unless `a == n`. `a` must be an ancestor of `n`.
    fn path_up(&self, mut n: usize, a: usize) -> Vec<usize> {
        let mut path = Vec::new();
        while n != a {
            path.push(n);
            n = self.revisions[n].parent;
        }
        path
    }

    /// Return the text steps needed to reach a revision, including across branches.
    fn jump_to(&mut self, to: usize) -> Vec<HistoryStep> {
        let steps = self.steps_to(to);
        self.current = to;
        steps
    }

    fn steps_to(&self, to: usize) -> Vec<HistoryStep> {
        let lca = self.lowest_common_ancestor(self.current, to);
        let up = self.path_up(self.current, lca);
        let down = self.path_up(to, lca);
        let up_txns = up.iter().map(|&n| self.step(n, HistoryDirection::Undo));
        let down_txns = down
            .iter()
            .rev()
            .map(|&n| self.step(n, HistoryDirection::Redo));
        up_txns.chain(down_txns).collect()
    }

    /// Helper for a binary search case below.
    fn revision_closer_to_instant(&self, i: usize, instant: Instant) -> usize {
        let dur_im1 = instant.duration_since(self.revisions[i - 1].timestamp);
        let dur_i = self.revisions[i].timestamp.duration_since(instant);
        use std::cmp::Ordering::*;
        match dur_im1.cmp(&dur_i) {
            Less => i - 1,
            Equal | Greater => i,
        }
    }

    /// Find the revision created at around
    /// `instant`.
    fn revision_at_instant(&self, instant: Instant) -> usize {
        let search_result = self
            .revisions
            .binary_search_by(|rev| rev.timestamp.cmp(&instant));
        match search_result {
            Ok(revision) => revision,
            Err(insert_point) => match insert_point {
                0 => 0,
                n if n == self.revisions.len() => n - 1,
                i => self.revision_closer_to_instant(i, instant),
            },
        }
    }

    /// Find the revision created `duration` ago
    /// from the timestamp of current revision.
    fn revision_duration_backward(&self, duration: Duration) -> usize {
        match self.revisions[self.current].timestamp.checked_sub(duration) {
            Some(instant) => self.revision_at_instant(instant),
            None => 0,
        }
    }

    /// Find the revision created `duration` in
    /// the future from the timestamp of the current revision.
    fn revision_duration_forward(&self, duration: Duration) -> usize {
        match self.revisions[self.current].timestamp.checked_add(duration) {
            Some(instant) => self.revision_at_instant(instant),
            None => self.revisions.len() - 1,
        }
    }

    fn target_for(&self, action: HistoryAction) -> usize {
        match action {
            HistoryAction::Undo => self.revisions[self.current].parent,
            HistoryAction::Redo => self.revisions[self.current]
                .last_child
                .map_or(self.current, NonZeroUsize::get),
            HistoryAction::Earlier(UndoKind::Steps(n)) => self.current.saturating_sub(n),
            HistoryAction::Later(UndoKind::Steps(n)) => {
                self.current.saturating_add(n).min(self.revisions.len() - 1)
            }
            HistoryAction::Earlier(UndoKind::TimePeriod(duration)) => {
                self.revision_duration_backward(duration)
            }
            HistoryAction::Later(UndoKind::TimePeriod(duration)) => {
                self.revision_duration_forward(duration)
            }
        }
    }

    /// Plan navigation without changing the current revision. Replay clients can
    /// apply each step with its text change before notifying observers.
    pub fn plan(&self, action: HistoryAction) -> Vec<HistoryStep> {
        self.steps_to(self.target_for(action))
    }

    /// Advance one step obtained from `plan`, rejecting stale or altered steps.
    /// The caller validates and applies the corresponding text change as part of
    /// the same operation, without publishing an intermediate state.
    pub fn advance(&mut self, step: &HistoryStep) -> bool {
        let Some(revision) = self.revisions.get(step.revision) else {
            return false;
        };
        if step.revision == 0 {
            return false;
        }
        let next = match step.direction {
            HistoryDirection::Undo
                if self.current == step.revision
                    && revision.inversion.matches(step.transaction.changes()) =>
            {
                revision.parent
            }
            HistoryDirection::Redo
                if revision.parent == self.current
                    && step.transaction.changes() == revision.transaction.changes() =>
            {
                step.revision
            }
            _ => return false,
        };
        self.current = next;
        true
    }

    /// Navigate earlier by revision count or duration.
    pub fn earlier(&mut self, kind: UndoKind) -> Vec<HistoryStep> {
        self.jump_to(self.target_for(HistoryAction::Earlier(kind)))
    }

    /// Navigate later by revision count or duration.
    pub fn later(&mut self, kind: UndoKind) -> Vec<HistoryStep> {
        self.jump_to(self.target_for(HistoryAction::Later(kind)))
    }
}

/// Whether to undo by a number of edits or a duration of time.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum UndoKind {
    Steps(usize),
    TimePeriod(std::time::Duration),
}

/// A subset of systemd.time time span syntax units.
const TIME_UNITS: &[(&[&str], &str, u64)] = &[
    (&["seconds", "second", "sec", "s"], "seconds", 1),
    (&["minutes", "minute", "min", "m"], "minutes", 60),
    (&["hours", "hour", "hr", "h"], "hours", 60 * 60),
    (&["days", "day", "d"], "days", 24 * 60 * 60),
];

/// Checks if the duration input can be turned into a valid duration. It must be a
/// positive integer and denote the [unit of time.](`TIME_UNITS`)
/// Examples of valid durations:
///  * `5 sec`
///  * `5 min`
///  * `5 hr`
///  * `5 days`
static DURATION_VALIDATION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\d+\s*[a-z]+\s*)+$").unwrap());

/// Captures both the number and unit as separate capture groups.
static NUMBER_UNIT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)\s*([a-z]+)").unwrap());

/// Parse a string (e.g. "5 sec") and try to convert it into a [`Duration`].
fn parse_human_duration(s: &str) -> Result<Duration, String> {
    if !DURATION_VALIDATION_REGEX.is_match(s) {
        return Err("duration should be composed \
        of positive integers followed by time units"
            .to_string());
    }

    let mut specified = [false; TIME_UNITS.len()];
    let mut seconds = 0u64;
    for cap in NUMBER_UNIT_REGEX.captures_iter(s) {
        let (n, unit_str) = (&cap[1], &cap[2]);

        let n: u64 = n.parse().map_err(|_| format!("integer too large: {}", n))?;

        let time_unit = TIME_UNITS
            .iter()
            .enumerate()
            .find(|(_, (forms, _, _))| forms.iter().any(|f| f == &unit_str));

        if let Some((i, (_, unit, mul))) = time_unit {
            if specified[i] {
                return Err(format!("{} specified more than once", unit));
            }
            specified[i] = true;

            let new_seconds = n.checked_mul(*mul).and_then(|s| seconds.checked_add(s));
            match new_seconds {
                Some(ns) => seconds = ns,
                None => return Err("duration too large".to_string()),
            }
        } else {
            return Err(format!("incorrect time unit: {}", unit_str));
        }
    }

    Ok(Duration::from_secs(seconds))
}

impl std::str::FromStr for UndoKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            Ok(Self::Steps(1usize))
        } else if let Ok(n) = s.parse::<usize>() {
            Ok(UndoKind::Steps(n))
        } else {
            Ok(Self::TimePeriod(parse_human_duration(s)?))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn large_inverse_payloads_share_storage_and_preserve_replay_and_branching() {
        use editor_core::ropey::extra::esoterica::ropes_are_instances;
        let original = Rope::from("α😀界 words\r\n".repeat(20_000));
        let mut state = original.clone();
        let mut history = History::default();
        let change = Transaction::change(
            &state,
            [(0, state.len_chars(), Some("replacement".into()))].into_iter(),
        );
        let revision = history.commit_revision(change.changes(), &state);
        let Inversion::Shared { operations, .. } = &history.revisions[revision].inversion else {
            panic!("large deletion must share")
        };
        assert!(operations.iter().any(|op| matches!(op, InverseOperation::Insert(text) if ropes_are_instances(text, &original))));
        assert!(change.apply(&mut state));
        let undo = history.plan(HistoryAction::Undo).pop().unwrap();
        let mut altered = undo.clone();
        let mut wrong = original.clone();
        wrong.remove(0.."α".len());
        wrong.insert(0, "β");
        altered.transaction = Transaction::change(
            &state,
            [(0, state.len_chars(), Some(wrong.to_string().into()))].into_iter(),
        );
        assert!(!history.advance(&altered));
        assert!(history.advance(&undo));
        assert!(undo.transaction.apply(&mut state));
        assert_eq!(state, original);
        let redo = history.redo().unwrap();
        assert!(redo.transaction.apply(&mut state));
        assert_eq!(state, "replacement");
        let inverse = history.changes_since(0).unwrap().invert(&original);
        assert!(inverse.apply(&mut state));
        assert_eq!(state, original);
        let undo = history.undo().unwrap();
        let mut deleted_again = Rope::from("replacement");
        assert!(undo.transaction.apply(&mut deleted_again));
        assert_eq!(deleted_again, original);
        let branch = Transaction::change(&state, [(0, state.len_chars(), None)].into_iter());
        history.commit_revision(branch.changes(), &state);
        branch.apply(&mut state);
        let rewind = history.earlier(UndoKind::Steps(1));
        for step in rewind {
            assert!(step.transaction.apply(&mut state));
        }
        assert_eq!(state, "replacement");
        let undo = history.undo().unwrap();
        assert!(undo.transaction.apply(&mut state));
        assert_eq!(state, original);
    }

    #[test]
    fn small_deletions_and_large_deletions_of_small_fractions_are_copied() {
        let original = Rope::from("a".repeat(1_000_000));
        for end in [1, 100_000] {
            let change = Transaction::change(&original, [(0, end, None)].into_iter());
            assert!(matches!(
                Inversion::new(change.changes(), &original),
                Inversion::Copied(_)
            ));
        }
        // Multiple distant deletions share collectively, with retained text and replacements.
        let change = Transaction::change(
            &original,
            [(0, 300_000, Some("界".into())), (500_000, 900_000, None)].into_iter(),
        );
        let inversion = Inversion::new(change.changes(), &original);
        assert!(matches!(inversion, Inversion::Shared { .. }));
        let inverse = inversion.transaction();
        assert!(inversion.matches(inverse.changes()));
        assert_eq!(inverse.changes(), change.invert(&original).changes());
        let mut state = original.clone();
        assert!(change.apply(&mut state));
        assert!(inverse.apply(&mut state));
        assert_eq!(state, original);
    }

    #[test]
    fn test_undo_redo() {
        let mut history = History::default();
        let doc = Rope::from("hello");
        let mut state = doc;

        let transaction1 =
            Transaction::change(&state, vec![(5, 5, Some(" world!".into()))].into_iter());

        // Need to commit before applying!
        history.commit_revision(transaction1.changes(), &state);
        transaction1.apply(&mut state);
        assert_eq!("hello world!", state);

        // ---

        let transaction2 =
            Transaction::change(&state, vec![(6, 11, Some("世界".into()))].into_iter());

        // Need to commit before applying!
        history.commit_revision(transaction2.changes(), &state);
        transaction2.apply(&mut state);
        assert_eq!("hello 世界!", state);

        // ---
        fn undo(history: &mut History, state: &mut Rope) {
            if let Some(step) = history.undo() {
                step.transaction.apply(state);
            }
        }
        fn redo(history: &mut History, state: &mut Rope) {
            if let Some(step) = history.redo() {
                step.transaction.apply(state);
            }
        }

        undo(&mut history, &mut state);
        assert_eq!("hello world!", state);
        redo(&mut history, &mut state);
        assert_eq!("hello 世界!", state);
        undo(&mut history, &mut state);
        undo(&mut history, &mut state);
        assert_eq!("hello", state);

        // undo at root is a no-op
        undo(&mut history, &mut state);
        assert_eq!("hello", state);
    }

    #[test]
    fn test_earlier_later() {
        let mut history = History::default();
        let doc = Rope::from("a\n");
        let mut state = doc;

        fn undo(history: &mut History, state: &mut Rope) {
            if let Some(step) = history.undo() {
                step.transaction.apply(state);
            }
        }

        fn earlier(history: &mut History, state: &mut Rope, uk: UndoKind) {
            let txns = history.earlier(uk);
            for txn in txns {
                txn.transaction.apply(state);
            }
        }

        fn later(history: &mut History, state: &mut Rope, uk: UndoKind) {
            let txns = history.later(uk);
            for txn in txns {
                txn.transaction.apply(state);
            }
        }

        fn commit_change(
            history: &mut History,
            state: &mut Rope,
            change: editor_core::Change,
            instant: Instant,
        ) {
            let txn = Transaction::change(state, vec![change].into_iter());
            history.commit_revision_at_timestamp(txn.changes(), state, instant);
            txn.apply(state);
        }

        let t0 = Instant::now();
        let t = |n| t0.checked_add(Duration::from_secs(n)).unwrap();

        commit_change(&mut history, &mut state, (1, 1, Some(" b".into())), t(0));
        assert_eq!("a b\n", state);

        commit_change(&mut history, &mut state, (3, 3, Some(" c".into())), t(10));
        assert_eq!("a b c\n", state);

        commit_change(&mut history, &mut state, (5, 5, Some(" d".into())), t(20));
        assert_eq!("a b c d\n", state);

        undo(&mut history, &mut state);
        assert_eq!("a b c\n", state);

        commit_change(&mut history, &mut state, (5, 5, Some(" e".into())), t(30));
        assert_eq!("a b c e\n", state);

        undo(&mut history, &mut state);
        undo(&mut history, &mut state);
        assert_eq!("a b\n", state);

        commit_change(&mut history, &mut state, (1, 3, None), t(40));
        assert_eq!("a\n", state);

        commit_change(&mut history, &mut state, (1, 1, Some(" f".into())), t(50));
        assert_eq!("a f\n", state);

        use UndoKind::*;

        earlier(&mut history, &mut state, Steps(3));
        assert_eq!("a b c d\n", state);

        later(&mut history, &mut state, TimePeriod(Duration::new(20, 0)));
        assert_eq!("a\n", state);

        earlier(&mut history, &mut state, TimePeriod(Duration::new(19, 0)));
        assert_eq!("a b c d\n", state);

        earlier(
            &mut history,
            &mut state,
            TimePeriod(Duration::new(10000, 0)),
        );
        assert_eq!("a\n", state);

        later(&mut history, &mut state, Steps(50));
        assert_eq!("a f\n", state);

        earlier(&mut history, &mut state, Steps(4));
        assert_eq!("a b c\n", state);

        later(&mut history, &mut state, TimePeriod(Duration::new(1, 0)));
        assert_eq!("a b c\n", state);

        later(&mut history, &mut state, TimePeriod(Duration::new(5, 0)));
        assert_eq!("a b c d\n", state);

        later(&mut history, &mut state, TimePeriod(Duration::new(6, 0)));
        assert_eq!("a b c e\n", state);

        later(&mut history, &mut state, Steps(1));
        assert_eq!("a\n", state);
    }

    #[test]
    fn test_parse_undo_kind() {
        use UndoKind::*;

        // Default is one step.
        assert_eq!("".parse(), Ok(Steps(1)));

        // An integer means the number of steps.
        assert_eq!("1".parse(), Ok(Steps(1)));
        assert_eq!("  16 ".parse(), Ok(Steps(16)));

        // Duration has a strict format.
        let validation_err = Err("duration should be composed \
         of positive integers followed by time units"
            .to_string());
        assert_eq!("  16 33".parse::<UndoKind>(), validation_err);
        assert_eq!("  seconds 22  ".parse::<UndoKind>(), validation_err);
        assert_eq!("  -4 m".parse::<UndoKind>(), validation_err);
        assert_eq!("5s 3".parse::<UndoKind>(), validation_err);

        // Units are u64.
        assert_eq!(
            "18446744073709551616minutes".parse::<UndoKind>(),
            Err("integer too large: 18446744073709551616".to_string())
        );

        // Units are validated.
        assert_eq!(
            "1 millennium".parse::<UndoKind>(),
            Err("incorrect time unit: millennium".to_string())
        );

        // Units can't be specified twice.
        assert_eq!(
            "2 seconds 6s".parse::<UndoKind>(),
            Err("seconds specified more than once".to_string())
        );

        // Various formats are correctly handled.
        assert_eq!(
            "4s".parse::<UndoKind>(),
            Ok(TimePeriod(Duration::from_secs(4)))
        );
        assert_eq!(
            "2m".parse::<UndoKind>(),
            Ok(TimePeriod(Duration::from_secs(120)))
        );
        assert_eq!(
            "5h".parse::<UndoKind>(),
            Ok(TimePeriod(Duration::from_secs(5 * 60 * 60)))
        );
        assert_eq!(
            "3d".parse::<UndoKind>(),
            Ok(TimePeriod(Duration::from_secs(3 * 24 * 60 * 60)))
        );
        assert_eq!(
            "1m30s".parse::<UndoKind>(),
            Ok(TimePeriod(Duration::from_secs(90)))
        );
        assert_eq!(
            "1m 20 seconds".parse::<UndoKind>(),
            Ok(TimePeriod(Duration::from_secs(80)))
        );
        assert_eq!(
            "  2 minute 1day".parse::<UndoKind>(),
            Ok(TimePeriod(Duration::from_secs(24 * 60 * 60 + 2 * 60)))
        );
        assert_eq!(
            "3 d 2hour 5 minutes 30sec".parse::<UndoKind>(),
            Ok(TimePeriod(Duration::from_secs(
                3 * 24 * 60 * 60 + 2 * 60 * 60 + 5 * 60 + 30
            )))
        );

        // Sum overflow is handled.
        assert_eq!(
            "18446744073709551615minutes".parse::<UndoKind>(),
            Err("duration too large".to_string())
        );
        assert_eq!(
            "1 minute 18446744073709551615 seconds".parse::<UndoKind>(),
            Err("duration too large".to_string())
        );
    }
}
