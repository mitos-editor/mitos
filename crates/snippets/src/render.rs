use std::borrow::Cow;
use std::ops::{Index, IndexMut};
use std::sync::Arc;
use stdx::rope::RopeExt as _;

use ropey::{Rope, RopeSlice};
use smallvec::SmallVec;
use stdx::Range;

use crate::elaborate;
use crate::TabstopIdx;
use crate::{Snippet, SnippetElement, Transform};
use editor_core::indent::{normalize_indentation, IndentStyle};
use editor_core::movement::Direction;
use editor_core::{selection, Selection, Tendril, Transaction};

#[derive(Debug, Clone, PartialEq)]
/// Render-time tabstop behavior.
pub enum TabstopKind {
    Choice { choices: Arc<[Tendril]> },
    Placeholder,
    Empty,
    Transform(Arc<Transform>),
}

#[derive(Debug, Clone, PartialEq)]
/// All rendered document ranges belonging to one logical tabstop.
pub struct Tabstop {
    /// Ranges are grouped by rendered snippet instance and stored in document
    /// character indices.
    pub ranges: SmallVec<[Range; 1]>,
    /// Original snippet instance owning each range, including conditional defaults.
    pub instances: SmallVec<[usize; 1]>,
    /// Parent tabstop for a nested placeholder.
    pub parent: Option<TabstopIdx>,
    /// Editing behavior of the tabstop.
    pub kind: TabstopKind,
}

impl Tabstop {
    /// Retain ranges together with their owning snippet instances.
    pub(super) fn retain_ranges(&mut self, mut keep: impl FnMut(&mut Range, usize) -> bool) {
        let mut index = 0;
        let mut instances = SmallVec::new();
        self.ranges.retain_mut(|range| {
            let instance = self.instances[index];
            index += 1;
            if keep(range, instance) {
                instances.push(instance);
                true
            } else {
                false
            }
        });
        self.instances = instances;
    }

    /// Returns whether entering the tabstop should initially select its text.
    pub fn has_placeholder(&self) -> bool {
        matches!(
            self.kind,
            TabstopKind::Choice { .. } | TabstopKind::Placeholder
        )
    }

    /// Builds the multi-selection for this tabstop.
    ///
    /// `primary_idx` identifies the primary rendered snippet instance, not a
    /// raw range within this tabstop.
    pub fn selection(&self, direction: Direction, primary_idx: usize) -> Selection {
        Selection::new(
            self.ranges
                .iter()
                .map(|&range| {
                    let mut range = selection::Range::new(range.start, range.end);
                    if direction == Direction::Backward {
                        range = range.flip()
                    }
                    range
                })
                .collect(),
            self.instances
                .iter()
                .position(|&instance| instance >= primary_idx)
                .unwrap_or(self.ranges.len() - 1),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
/// Output metadata accumulated while inserting a snippet at one or more cursors.
pub struct RenderedSnippet {
    /// Tabstops in navigation order, including the final stop.
    pub tabstops: Vec<Tabstop>,
    /// Outer range of each rendered snippet instance.
    pub ranges: Vec<Range>,
}

impl RenderedSnippet {
    /// Selects the first navigable tabstop after insertion.
    pub fn first_selection(&self, direction: Direction, primary_idx: usize) -> Selection {
        self.tabstops[0].selection(direction, primary_idx)
    }
}

impl Index<TabstopIdx> for RenderedSnippet {
    type Output = Tabstop;
    fn index(&self, index: TabstopIdx) -> &Tabstop {
        &self.tabstops[index.0]
    }
}

impl IndexMut<TabstopIdx> for RenderedSnippet {
    fn index_mut(&mut self, index: TabstopIdx) -> &mut Tabstop {
        &mut self.tabstops[index.0]
    }
}

impl Snippet {
    /// Allocates empty render metadata matching this snippet's tabstop model.
    pub fn prepare_render(&self) -> RenderedSnippet {
        let tabstops =
            self.tabstops()
                .map(|tabstop| Tabstop {
                    ranges: SmallVec::new(),
                    instances: SmallVec::new(),
                    parent: tabstop.parent,
                    kind: match &tabstop.kind {
                        elaborate::TabstopKind::Choice { choices } => TabstopKind::Choice {
                            choices: choices.clone(),
                        },
                        // start out as empty: the first non-empty placeholder will change this to
                        // a placeholder automatically
                        elaborate::TabstopKind::Empty
                        | elaborate::TabstopKind::Placeholder { .. } => TabstopKind::Empty,
                        elaborate::TabstopKind::Transform(transform) => {
                            TabstopKind::Transform(transform.clone())
                        }
                    },
                })
                .collect();
        RenderedSnippet {
            tabstops,
            ranges: Vec::new(),
        }
    }

    /// Renders one snippet instance at a document character position.
    ///
    /// Returns the inserted text and its length in characters. `indent` is the
    /// existing line indentation copied after embedded newlines.
    pub fn render_at(
        &self,
        snippet: &mut RenderedSnippet,
        indent: RopeSlice<'_>,
        at_newline: bool,
        ctx: &mut SnippetRenderCtx,
        pos: usize,
        cursor_index: usize,
    ) -> (Tendril, usize) {
        let mut ctx = SnippetRender {
            dst: snippet,
            src: self,
            indent,
            text: Tendril::new(),
            off: pos,
            ctx,
            at_newline,
            cursor_index,
        };
        ctx.render_elements(self.elements());
        let end = ctx.off;
        let text = ctx.text;
        snippet.ranges.push(Range { start: pos, end });
        (text, end - pos)
    }

    /// Renders the snippet for each selected range.
    ///
    /// Returns, in order, the document transaction, post-change selection, and
    /// metadata needed to begin an [`crate::ActiveSnippet`] session.
    pub fn render(
        &self,
        doc: &Rope,
        selection: &Selection,
        mut change_range: impl FnMut(&selection::Range) -> (usize, usize),
        ctx: &mut SnippetRenderCtx,
    ) -> (Transaction, Selection, RenderedSnippet) {
        let mut instances = Vec::new();
        let mut off = 0;
        let mut ranges = SmallVec::new();
        let mut primary_index = 0;
        let transaction = Transaction::change_ignore_overlapping(
            doc,
            selection.iter().enumerate().map(|(index, range)| {
                let (start, end) = change_range(range);
                (start, end, (index, range))
            }),
            |replacement_start, replacement_end, (index, range)| {
                if index <= selection.primary_index() {
                    primary_index = ranges.len();
                }
                ranges.push(*range);
                let line_idx = doc.char_to_line_idx(replacement_start);
                let line_start = doc.line_to_char_idx(line_idx);
                let prefix = doc.slice_chars(line_start..replacement_start);
                let indent_len = prefix.chars().take_while(|c| c.is_whitespace()).count();
                let indent = prefix.slice_chars(..indent_len);
                let at_newline = indent_len == replacement_start - line_start;

                let mut instance = self.prepare_render();
                let (replacement, replacement_len) = self.render_at(
                    &mut instance,
                    indent,
                    at_newline,
                    ctx,
                    (replacement_start as i128 + off) as usize,
                    index,
                );
                instances.push(instance);
                off +=
                    replacement_start as i128 - replacement_end as i128 + replacement_len as i128;

                Some(replacement)
            },
        );
        (
            transaction,
            Selection::new(ranges, primary_index),
            merge_instances(instances),
        )
    }
}

// A variable can resolve at one cursor but use a default containing placeholders
// at another. Track each range's owner explicitly, and omit tabstops whose
// conditional defaults were skipped everywhere.
fn merge_instances(instances: Vec<RenderedSnippet>) -> RenderedSnippet {
    let first = &instances[0];
    let mut indices = vec![None; first.tabstops.len()];
    let mut count = 0;
    for (index, mapped) in indices.iter_mut().enumerate() {
        if index == first.tabstops.len() - 1
            || instances
                .iter()
                .any(|instance| !instance.tabstops[index].ranges.is_empty())
        {
            *mapped = Some(TabstopIdx(count));
            count += 1;
        }
    }
    let mut tabstops = Vec::with_capacity(count);
    for (index, mapped) in indices.iter().enumerate() {
        if mapped.is_none() {
            continue;
        }
        let mut tabstop = first.tabstops[index].clone();
        tabstop.ranges.clear();
        tabstop.instances.clear();
        while let Some(parent) = tabstop.parent {
            if let Some(mapped) = indices[parent.0] {
                tabstop.parent = Some(mapped);
                break;
            }
            tabstop.parent = first.tabstops[parent.0].parent;
        }
        for (instance_index, instance) in instances.iter().enumerate() {
            let source = &instance.tabstops[index];
            if source.has_placeholder() {
                tabstop.kind = source.kind.clone();
            }
            tabstop.ranges.extend_from_slice(&source.ranges);
            tabstop
                .instances
                .extend(std::iter::repeat_n(instance_index, source.ranges.len()));
            // A final stop nested in a skipped variable default still ends the snippet.
            if index == first.tabstops.len() - 1 && source.ranges.is_empty() {
                let end = instance.ranges[0].end;
                tabstop.ranges.push(Range { start: end, end });
                tabstop.instances.push(instance_index);
            }
        }
        tabstops.push(tabstop);
    }
    RenderedSnippet {
        tabstops,
        ranges: instances
            .into_iter()
            .flat_map(|instance| instance.ranges)
            .collect(),
    }
}

/// Callback used to resolve an LSP snippet variable by name.
///
/// The second argument is the original selection index, before overlapping edits
/// are removed or earlier insertions shift output positions.
/// Returning `None` causes the variable's default value, if any, to render.
pub type VariableResolver = dyn FnMut(&str, usize) -> Option<Cow<str>>;
/// Document-specific settings needed while rendering a snippet.
pub struct SnippetRenderCtx {
    /// Resolver for editor and protocol variables such as `TM_FILENAME`.
    pub resolve_var: Box<VariableResolver>,
    /// Display width used when normalizing indentation.
    pub tab_width: usize,
    /// Indentation style of the destination document.
    pub indent_style: IndentStyle,
    /// Line-ending sequence of the destination document.
    pub line_ending: &'static str,
}

impl SnippetRenderCtx {
    #[cfg(test)]
    pub(super) fn test_ctx() -> SnippetRenderCtx {
        SnippetRenderCtx {
            resolve_var: Box::new(|_, _| None),
            tab_width: 4,
            indent_style: IndentStyle::Spaces(4),
            line_ending: "\n",
        }
    }
}

struct SnippetRender<'a> {
    ctx: &'a mut SnippetRenderCtx,
    dst: &'a mut RenderedSnippet,
    src: &'a Snippet,
    indent: RopeSlice<'a>,
    text: Tendril,
    off: usize,
    at_newline: bool,
    cursor_index: usize,
}

impl SnippetRender<'_> {
    fn render_elements(&mut self, elements: &[SnippetElement]) {
        for element in elements {
            self.render_element(element)
        }
    }

    fn render_element(&mut self, element: &SnippetElement) {
        match *element {
            SnippetElement::Tabstop { idx } => self.render_tabstop(idx),
            SnippetElement::Variable {
                ref name,
                ref default,
                ref transform,
            } => {
                if let Some(val) = (self.ctx.resolve_var)(name, self.cursor_index) {
                    if let Some(transform) = transform {
                        self.push_multiline_str(&transform.apply(
                            (&*val).into(),
                            Range {
                                start: 0,
                                end: val.chars().count(),
                            },
                        ));
                    } else {
                        self.push_multiline_str(&val)
                    }
                } else if let Some(default) = default {
                    self.render_elements(default)
                } else if let Some(transform) = transform {
                    self.push_multiline_str(
                        &transform.apply("".into(), Range { start: 0, end: 0 }),
                    );
                }
            }
            SnippetElement::Text(ref text) => self.push_multiline_str(text),
        }
    }

    fn push_multiline_str(&mut self, text: &str) {
        let mut lines = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line));
        let first_line = lines.next().unwrap();
        self.push_str(first_line, self.at_newline);
        for line in lines {
            self.push_newline();
            self.push_str(line, true);
        }
    }

    fn push_str(&mut self, mut text: &str, at_newline: bool) {
        if at_newline {
            let old_len = self.text.len();
            let old_indent_len = normalize_indentation(
                self.indent,
                text.into(),
                &mut self.text,
                self.ctx.indent_style,
                self.ctx.tab_width,
            );
            // this is ok because indentation can only be ascii chars (' ' and '\t')
            self.off += self.text.len() - old_len;
            text = &text[old_indent_len..];
            if text.is_empty() {
                self.at_newline = true;
                return;
            }
        }
        self.text.push_str(text);
        self.off += text.chars().count();
    }

    fn push_newline(&mut self) {
        self.off += self.ctx.line_ending.chars().count() + self.indent.len_chars();
        self.text.push_str(self.ctx.line_ending);
        self.text.extend(self.indent.chunks());
    }

    fn render_tabstop(&mut self, tabstop: TabstopIdx) {
        let start = self.off;
        let end = match &self.src[tabstop].kind {
            elaborate::TabstopKind::Placeholder { default } if !default.is_empty() => {
                self.render_elements(default);
                self.dst[tabstop].kind = TabstopKind::Placeholder;
                self.off
            }
            elaborate::TabstopKind::Choice { choices } => {
                if let Some(first) = choices.first() {
                    self.push_multiline_str(first);
                }
                self.off
            }
            _ => start,
        };
        self.dst[tabstop].ranges.push(Range { start, end });
        self.dst[tabstop].instances.push(0);
    }
}

#[cfg(test)]
mod tests {
    use editor_core::{Rope, Selection};
    use stdx::Range;

    use crate::render::Tabstop;
    use crate::{Snippet, SnippetRenderCtx};

    use super::TabstopKind;

    fn assert_snippet(snippet: &str, expect: &str, tabstops: &[Tabstop]) {
        let snippet = Snippet::parse(snippet).unwrap();
        let mut rendered_snippet = snippet.prepare_render();
        let rendered_text = snippet
            .render_at(
                &mut rendered_snippet,
                "\t".into(),
                false,
                &mut SnippetRenderCtx::test_ctx(),
                0,
                0,
            )
            .0;
        assert_eq!(rendered_text, expect);
        assert_eq!(&rendered_snippet.tabstops, tabstops);
        assert_eq!(
            rendered_snippet.ranges.last().unwrap().end,
            rendered_text.chars().count()
        );
        assert_eq!(rendered_snippet.ranges.last().unwrap().start, 0)
    }

    #[test]
    fn a_bare_reference_before_a_default_uses_the_default_everywhere() {
        let snippet = Snippet::parse("$1 ${1:name} $1$0").unwrap();
        let (edit, _, rendered) = snippet.render(
            &Rope::from("\n"),
            &Selection::point(0),
            |_| (0, 0),
            &mut SnippetRenderCtx::test_ctx(),
        );
        let mut text = Rope::from("\n");
        edit.apply(&mut text);
        assert_eq!(text, "name name name\n");
        assert_eq!(rendered.tabstops[0].ranges.len(), 3);
    }

    #[test]
    fn rust_macro() {
        assert_snippet(
            "macro_rules! ${1:name} {\n\t($3) => {\n\t\t$2\n\t};\n}",
            "macro_rules! name {\n\t    () => {\n\t        \n\t    };\n\t}",
            &[
                Tabstop {
                    ranges: vec![Range { start: 13, end: 17 }].into(),
                    parent: None,
                    instances: vec![0].into(),
                    kind: TabstopKind::Placeholder,
                },
                Tabstop {
                    ranges: vec![Range { start: 42, end: 42 }].into(),
                    parent: None,
                    instances: vec![0].into(),
                    kind: TabstopKind::Empty,
                },
                Tabstop {
                    ranges: vec![Range { start: 26, end: 26 }].into(),
                    parent: None,
                    instances: vec![0].into(),
                    kind: TabstopKind::Empty,
                },
                Tabstop {
                    ranges: vec![Range { start: 53, end: 53 }].into(),
                    parent: None,
                    instances: vec![0].into(),
                    kind: TabstopKind::Empty,
                },
            ],
        );
    }
}

#[cfg(test)]
mod conditional_variable_tests {
    use super::*;

    #[test]
    fn resolved_variables_skip_placeholder_defaults_without_losing_the_final_cursor() {
        let source = Rope::from_str("\n");
        let mut context = SnippetRenderCtx::test_ctx();
        context.resolve_var = Box::new(|_, _| Some(Cow::Borrowed("file.rs")));
        for body in ["${TM_FILENAME:${1:name}}$0", "${TM_FILENAME:${0:name}}"] {
            let snippet = Snippet::parse(body).unwrap();
            let (transaction, _, rendered) =
                snippet.render(&source, &Selection::point(0), |_| (0, 0), &mut context);
            let mut text = source.clone();
            transaction.apply(&mut text);
            assert_eq!(text, "file.rs\n");
            assert_eq!(rendered.tabstops.len(), 1);
            assert_eq!(
                rendered.first_selection(Direction::Forward, 0),
                Selection::point(7)
            );
        }
    }

    #[test]
    fn conditional_placeholders_preserve_cursor_groups_and_primary_selection() {
        let source = Rope::from_str("\n\n");
        let selection = Selection::new(
            vec![selection::Range::point(0), selection::Range::point(1)].into(),
            1,
        );
        let mut context = SnippetRenderCtx::test_ctx();
        context.resolve_var = Box::new(|_, index| (index == 0).then_some(Cow::Borrowed("known")));
        let snippet = Snippet::parse("${VALUE:${1:name} $1}$0").unwrap();
        let (transaction, _, rendered) = snippet.render(
            &source,
            &selection,
            |range| (range.head, range.head),
            &mut context,
        );
        let mut text = source.clone();
        transaction.apply(&mut text);
        assert_eq!(text, "known\nname name\n");
        let selection = rendered.first_selection(Direction::Forward, 1);
        assert_eq!(selection.primary().from(), 6);
        assert_eq!(selection.primary().to(), 10);
        let mut active = crate::ActiveSnippet::new(rendered).unwrap();
        assert!(active.is_valid(&selection));
        let edit = Transaction::change_by_selection(&text, &selection, |range| {
            (range.from(), range.to(), Some("X".into()))
        });
        let selection = selection.map(edit.changes());
        edit.apply(&mut text);
        assert!(!active.map(edit.changes()));
        assert_eq!(text, "known\nX X\n");
        let (final_selection, finished) = active.next_tabstop(&selection);
        assert!(finished);
        assert_eq!(final_selection.len(), 2);
    }
}
