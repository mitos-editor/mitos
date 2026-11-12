//! Reuse imara's line interning without copying source text.
//!
//! Interned slices borrow boxed, immutable ropes whose addresses survive moving this cache.
//! Erase those slices before replacing either rope. Keep the interner private and expose it
//! only with a lifetime bounded by a borrow of the cache.
use std::mem::transmute;

use editor_core::{Rope, RopeSlice};
use gix_imara_diff::{InternedInput, Interner};

use super::{MAX_DIFF_BYTES, MAX_DIFF_LINES};

/// A cache that stores the `lines` of a rope as a vector.
/// It allows safely reusing the allocation of the vec when updating the rope
pub(super) struct InternedRopeLines {
    // Drop borrowed slices before their backing ropes.
    interned: InternedInput<RopeSlice<'static>>,
    diff_base: Box<Rope>,
    doc: Box<Rope>,
    /// None when size limits prevented interning the base.
    num_tokens_diff_base: Option<u32>,
}

impl InternedRopeLines {
    pub fn new(diff_base: Rope, doc: Rope) -> InternedRopeLines {
        let mut res = InternedRopeLines {
            interned: InternedInput {
                before: Vec::new(),
                after: Vec::new(),
                interner: Interner::new(0),
            },
            diff_base: Box::new(diff_base),
            doc: Box::new(doc),
            num_tokens_diff_base: None,
        };
        if !res.is_too_large() {
            res.interned.reserve(
                res.diff_base.len_lines(stdx::rope::LINE_TYPE) as u32,
                res.doc.len_lines(stdx::rope::LINE_TYPE) as u32,
            );
            res.update_diff_base_impl();
        }
        res
    }

    pub fn doc(&self) -> Rope {
        Rope::clone(&*self.doc)
    }

    pub fn diff_base(&self) -> Rope {
        Rope::clone(&*self.diff_base)
    }

    /// Updates the `diff_base` and optionally the document if `doc` is not None
    pub fn update_diff_base(&mut self, diff_base: Rope, doc: Option<Rope>) {
        self.interned.clear();
        self.num_tokens_diff_base = None;
        *self.diff_base = diff_base;
        if let Some(doc) = doc {
            *self.doc = doc
        }
        if !self.is_too_large() {
            self.update_diff_base_impl();
        }
    }

    /// Updates the `doc` without reinterning the `diff_base`, this function
    /// is therefore significantly faster than `update_diff_base` when only the document changes.
    pub fn update_doc(&mut self, doc: Rope) {
        // Safety: we clear any tokens that were added after
        // the interning of `self.diff_base` finished so
        // all lines that refer to `self.doc` have been purged.

        self.interned.after.clear();
        if let Some(tokens) = self.num_tokens_diff_base {
            self.interned.interner.erase_tokens_after(tokens.into());
        }

        *self.doc = doc;
        if !self.is_too_large() {
            if self.num_tokens_diff_base.is_none() {
                self.update_diff_base_impl();
            } else {
                self.update_doc_impl();
            }
        }
    }

    fn update_diff_base_impl(&mut self) {
        // SAFETY: The boxed base stays at a stable address and is never mutated. Its slices are
        // cleared before replacement and cannot escape the lifetime of a borrow of this cache.
        let before = self.diff_base.lines(stdx::rope::LINE_TYPE).map(
            |line: RopeSlice| -> RopeSlice<'static> {
                unsafe { transmute(super::line_token(line)) }
            },
        );
        self.interned.update_before(before);
        self.num_tokens_diff_base = Some(self.interned.interner.num_tokens());
        // The document must be interned again after clearing the interner.
        self.update_doc_impl()
    }

    fn update_doc_impl(&mut self) {
        // SAFETY: The boxed document stays at a stable address and is never mutated. Before
        // replacement, update_doc erases all tokens added after base interning, and
        // update_diff_base clears the whole interner. Slices cannot outlive a cache borrow.
        let after =
            self.doc
                .lines(stdx::rope::LINE_TYPE)
                .map(|line: RopeSlice| -> RopeSlice<'static> {
                    unsafe { transmute(super::line_token(line)) }
                });
        self.interned.update_after(after);
    }

    fn is_too_large(&self) -> bool {
        // bound both lines and bytes to avoid huge files with few (but huge) lines
        // or huge file with tiny lines. While this makes no difference to
        // diff itself (the diff performance only depends on the number of tokens)
        // the interning runtime depends mostly on filesize and is actually dominant
        // for large files
        self.doc.len_lines(stdx::rope::LINE_TYPE) > MAX_DIFF_LINES
            || self.diff_base.len_lines(stdx::rope::LINE_TYPE) > MAX_DIFF_LINES
            || self.doc.len() > MAX_DIFF_BYTES
            || self.diff_base.len() > MAX_DIFF_BYTES
    }

    /// Returns the `InternedInput` for performing the diff.
    /// If `diff_base` or `doc` is so large that performing a diff could slow the editor
    /// this function returns `None`.
    pub fn interned_lines(&self) -> Option<&InternedInput<RopeSlice<'_>>> {
        if self.is_too_large() {
            None
        } else {
            Some(&self.interned)
        }
    }
}
