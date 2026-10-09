use std::mem::swap;
use std::ops::Index;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use regex_cursor::engines::meta::Builder as RegexBuilder;
use regex_cursor::engines::meta::Regex;
use regex_cursor::regex_automata::util::syntax::Config as RegexConfig;
use ropey::RopeSlice;
use stdx::rope::RopeSliceExt;
use stdx::Range;

use crate::parser::{self, CaseChange, FormatItem};
use crate::{TabstopIdx, LAST_TABSTOP_IDX};
use editor_core::case_conversion::to_lower_case_with;
use editor_core::case_conversion::to_upper_case_with;
use editor_core::case_conversion::{to_camel_case_with, to_pascal_case_with};
use editor_core::Tendril;

#[derive(Debug)]
/// Parsed snippet with normalized tabstop ordering and nesting.
pub struct Snippet {
    elements: Vec<SnippetElement>,
    tabstops: Vec<Tabstop>,
}

#[cfg(test)]
mod static_tests {
    use super::*;
    #[test]
    fn static_render_preflight_bounds_expanded_defaults_and_rejects_transforms() {
        let repeated = Snippet::parse("${1:hello}$1$1").unwrap();
        assert_eq!(repeated.static_render_size(32), Some((15, 0, 7)));
        assert!(repeated.static_render_size(10).is_none());
        let transformed = Snippet::parse("${TM_FILENAME/hello/world/}").unwrap();
        assert!(
            transformed.static_render_size(64).is_none(),
            "{transformed:?}"
        );
    }
}

impl Snippet {
    /// Parses LSP snippet syntax and elaborates it into the editing model.
    ///
    /// The error includes the unconsumed suffix at which parsing stopped.
    pub fn parse(snippet: &str) -> Result<Self> {
        let parsed_snippet = parser::parse(snippet)
            .map_err(|rest| anyhow!("Failed to parse snippet. Remaining input: {}", rest))?;
        Ok(Snippet::new(parsed_snippet))
    }

    /// Elaborates parser elements into a snippet ready for rendering.
    ///
    /// Duplicate tabstop labels are merged, missing `$0` is appended, and
    /// source labels are replaced with dense [`TabstopIdx`] values.
    pub fn new(elements: Vec<parser::SnippetElement>) -> Snippet {
        let mut res = Snippet {
            elements: Vec::new(),
            tabstops: Vec::new(),
        };
        res.elements = res.elaborate(elements, None).into();
        res.fixup_tabstops();
        res.ensure_last_tabstop();
        res.renumber_tabstops();
        res
    }

    /// Returns the normalized top-level elements.
    pub fn elements(&self) -> &[SnippetElement] {
        &self.elements
    }

    /// Iterates over tabstops in navigation order, ending with `$0`.
    pub fn tabstops(&self) -> impl Iterator<Item = &Tabstop> {
        self.tabstops.iter()
    }

    /// Preflight static, untrusted snippets before rendering. Bounds expanded
    /// placeholder defaults and rejects recursive tabstops and transforms whose
    /// output depends on runtime input. Native/LSP rendering is unchanged.
    pub fn static_render_size(&self, limit: usize) -> Option<(usize, usize, usize)> {
        if self
            .tabstops()
            .any(|tabstop| matches!(tabstop.kind, TabstopKind::Transform(_)))
        {
            return None;
        }
        fn measure(
            snippet: &Snippet,
            elements: &[SnippetElement],
            visiting: &mut Vec<TabstopIdx>,
            limit: usize,
            nodes: &mut usize,
        ) -> Option<(usize, usize)> {
            let mut size = 0usize;
            let mut lines = 0usize;
            for element in elements {
                *nodes = nodes.checked_add(1)?;
                if *nodes > limit {
                    return None;
                }
                let (bytes, newlines) = match element {
                    SnippetElement::Text(text) => (
                        text.len(),
                        text.chars().filter(|character| *character == '\n').count(),
                    ),
                    SnippetElement::Variable {
                        default,
                        transform: None,
                        ..
                    } => measure(
                        snippet,
                        default.as_deref().unwrap_or_default(),
                        visiting,
                        limit,
                        nodes,
                    )?,
                    SnippetElement::Variable {
                        transform: Some(_), ..
                    } => return None,
                    SnippetElement::Tabstop { idx } => {
                        if visiting.contains(idx) || visiting.len() >= 32 {
                            return None;
                        }
                        visiting.push(*idx);
                        let measured = match &snippet[*idx].kind {
                            TabstopKind::Placeholder { default } => {
                                measure(snippet, default, visiting, limit, nodes)?
                            }
                            TabstopKind::Choice { choices } => {
                                choices.iter().fold((0, 0), |(bytes, lines), choice| {
                                    (
                                        bytes.max(choice.len()),
                                        lines.max(
                                            choice
                                                .chars()
                                                .filter(|character| *character == '\n')
                                                .count(),
                                        ),
                                    )
                                })
                            }
                            TabstopKind::Empty => (0, 0),
                            TabstopKind::Transform(_) => return None,
                        };
                        visiting.pop();
                        measured
                    }
                };
                size = size.checked_add(bytes)?;
                lines = lines.checked_add(newlines)?;
                if size > limit {
                    return None;
                }
            }
            Some((size, lines))
        }
        let mut nodes = 0;
        let (bytes, lines) = measure(self, self.elements(), &mut Vec::new(), limit, &mut nodes)?;
        Some((bytes, lines, nodes))
    }

    fn renumber_tabstops(&mut self) {
        Self::renumber_tabstops_in(&self.tabstops, &mut self.elements);
        for i in 0..self.tabstops.len() {
            if let Some(parent) = self.tabstops[i].parent {
                let parent = self
                    .tabstops
                    .binary_search_by_key(&parent, |tabstop| tabstop.idx)
                    .expect("all tabstops have been resolved");
                self.tabstops[i].parent = Some(TabstopIdx(parent));
            }
            let tabstop = &mut self.tabstops[i];
            if let TabstopKind::Placeholder { default } = &tabstop.kind {
                let mut default = default.clone();
                tabstop.kind = TabstopKind::Empty;
                Self::renumber_tabstops_in(&self.tabstops, Arc::get_mut(&mut default).unwrap());
                self.tabstops[i].kind = TabstopKind::Placeholder { default };
            }
        }
    }

    fn renumber_tabstops_in(tabstops: &[Tabstop], elements: &mut [SnippetElement]) {
        for elem in elements {
            match elem {
                SnippetElement::Tabstop { idx } => {
                    idx.0 = tabstops
                        .binary_search_by_key(&*idx, |tabstop| tabstop.idx)
                        .expect("all tabstops have been resolved")
                }
                SnippetElement::Variable { default, .. } => {
                    if let Some(default) = default {
                        Self::renumber_tabstops_in(tabstops, default);
                    }
                }
                SnippetElement::Text(_) => (),
            }
        }
    }

    fn fixup_tabstops(&mut self) {
        self.tabstops.sort_by_key(|tabstop| tabstop.idx);
        self.tabstops.dedup_by(|tabstop1, tabstop2| {
            if tabstop1.idx != tabstop2.idx {
                return false;
            }
            // use the first non empty tabstop for all multicursor tabstops
            if tabstop2.kind.is_empty() {
                swap(tabstop2, tabstop1)
            }
            true
        })
    }

    fn ensure_last_tabstop(&mut self) {
        if matches!(self.tabstops.last(), Some(tabstop) if tabstop.idx == LAST_TABSTOP_IDX) {
            return;
        }
        self.tabstops.push(Tabstop {
            idx: LAST_TABSTOP_IDX,
            parent: None,
            kind: TabstopKind::Empty,
        });
        self.elements.push(SnippetElement::Tabstop {
            idx: LAST_TABSTOP_IDX,
        })
    }

    fn elaborate(
        &mut self,
        default: Vec<parser::SnippetElement>,
        parent: Option<TabstopIdx>,
    ) -> Box<[SnippetElement]> {
        default
            .into_iter()
            .map(|val| match val {
                parser::SnippetElement::Tabstop {
                    tabstop,
                    transform: None,
                } => SnippetElement::Tabstop {
                    idx: self.elaborate_placeholder(tabstop, parent, Vec::new()),
                },
                parser::SnippetElement::Tabstop {
                    tabstop,
                    transform: Some(transform),
                } => SnippetElement::Tabstop {
                    idx: self.elaborate_transform(tabstop, parent, transform),
                },
                parser::SnippetElement::Placeholder { tabstop, value } => SnippetElement::Tabstop {
                    idx: self.elaborate_placeholder(tabstop, parent, value),
                },
                parser::SnippetElement::Choice { tabstop, choices } => SnippetElement::Tabstop {
                    idx: self.elaborate_choice(tabstop, parent, choices),
                },
                parser::SnippetElement::Variable {
                    name,
                    default,
                    transform,
                } => SnippetElement::Variable {
                    name,
                    default: default.map(|default| self.elaborate(default, parent)),
                    // TODO: error for invalid transforms
                    transform: transform.and_then(Transform::new).map(Box::new),
                },
                parser::SnippetElement::Text(text) => SnippetElement::Text(text),
            })
            .collect()
    }

    fn elaborate_choice(
        &mut self,
        idx: usize,
        parent: Option<TabstopIdx>,
        choices: Vec<Tendril>,
    ) -> TabstopIdx {
        let idx = TabstopIdx::elaborate(idx);
        self.tabstops.push(Tabstop {
            idx,
            parent,
            kind: TabstopKind::Choice {
                choices: choices.into(),
            },
        });
        idx
    }

    fn elaborate_placeholder(
        &mut self,
        idx: usize,
        parent: Option<TabstopIdx>,
        mut default: Vec<parser::SnippetElement>,
    ) -> TabstopIdx {
        let idx = TabstopIdx::elaborate(idx);
        if idx == LAST_TABSTOP_IDX && !default.is_empty() {
            // Older versions of clangd for example may send a snippet like `${0:placeholder}`
            // which is considered by VSCode to be a misuse of the `$0` tabstop.
            log::warn!("Discarding placeholder text for the `$0` tabstop ({default:?}). \
                The `$0` tabstop signifies the final cursor position and should not include placeholder text.");
            default.clear();
        }
        let default = self.elaborate(default, Some(idx));
        self.tabstops.push(Tabstop {
            idx,
            parent,
            kind: TabstopKind::Placeholder {
                default: default.into(),
            },
        });
        idx
    }

    fn elaborate_transform(
        &mut self,
        idx: usize,
        parent: Option<TabstopIdx>,
        transform: parser::Transform,
    ) -> TabstopIdx {
        let idx = TabstopIdx::elaborate(idx);
        match Transform::new(transform) {
            Some(transform) => self.tabstops.push(Tabstop {
                idx,
                parent,
                kind: TabstopKind::Transform(Arc::new(transform)),
            }),
            _ => {
                // TODO: proper error
                self.tabstops.push(Tabstop {
                    idx,
                    parent,
                    kind: TabstopKind::Empty,
                })
            }
        }
        idx
    }
}

impl Index<TabstopIdx> for Snippet {
    type Output = Tabstop;
    fn index(&self, index: TabstopIdx) -> &Tabstop {
        &self.tabstops[index.0]
    }
}

#[derive(Debug)]
/// An element in an elaborated snippet.
pub enum SnippetElement {
    Tabstop {
        idx: TabstopIdx,
    },
    Variable {
        name: Tendril,
        default: Option<Box<[SnippetElement]>>,
        transform: Option<Box<Transform>>,
    },
    Text(Tendril),
}

#[derive(Debug)]
/// Metadata for one normalized tabstop.
pub struct Tabstop {
    idx: TabstopIdx,
    pub parent: Option<TabstopIdx>,
    pub kind: TabstopKind,
}

#[derive(Debug)]
/// Content or behavior associated with a normalized tabstop.
pub enum TabstopKind {
    Choice { choices: Arc<[Tendril]> },
    Placeholder { default: Arc<[SnippetElement]> },
    Empty,
    Transform(Arc<Transform>),
}

impl TabstopKind {
    /// Returns whether this tabstop inserts no initial content.
    pub fn is_empty(&self) -> bool {
        matches!(self, TabstopKind::Empty)
    }
}

#[derive(Debug)]
/// Compiled regular-expression transform attached to a tabstop or variable.
pub struct Transform {
    regex: Regex,
    regex_str: Box<str>,
    global: bool,
    replacement: Box<[FormatItem]>,
}

impl PartialEq for Transform {
    fn eq(&self, other: &Self) -> bool {
        self.replacement == other.replacement
            && self.global == other.global
            // doens't compare m and i setting but close enough
            && self.regex_str == other.regex_str
    }
}

impl Transform {
    fn new(transform: parser::Transform) -> Option<Transform> {
        let mut config = RegexConfig::new();
        let mut global = false;
        let mut invalid_config = false;
        for c in transform.options.chars() {
            match c {
                'i' => {
                    config = config.case_insensitive(true);
                }
                'm' => {
                    config = config.multi_line(true);
                }
                'g' => {
                    global = true;
                }
                // we ignore 'u' since we always want to
                // do unicode aware matching
                _ => invalid_config = true,
            }
        }
        if invalid_config {
            log::error!("invalid transform configuration characters {transform:?}");
        }
        let regex = match RegexBuilder::new().syntax(config).build(&transform.regex) {
            Ok(regex) => regex,
            Err(err) => {
                log::error!("invalid transform {err} {transform:?}");
                return None;
            }
        };
        Some(Transform {
            regex,
            regex_str: transform.regex.as_str().into(),
            global,
            replacement: transform.replacement.into(),
        })
    }

    /// Applies the transform to `range` within `doc`.
    ///
    /// The range is expressed in rope character indices. Regex capture ranges
    /// are byte offsets into that slice and are converted through `RopeSlice`
    /// before text is copied.
    pub fn apply(&self, mut doc: RopeSlice<'_>, range: Range) -> Tendril {
        let mut buf = Tendril::new();
        let it = self.regex.captures_iter(doc.regex_input_at(range));
        doc = doc.slice(range);
        let mut last_match = 0;
        for cap in it {
            // unwrap on 0 is OK because captures only reports matches
            let m = cap.get_group(0).unwrap();
            buf.extend(doc.byte_slice(last_match..m.start).chunks());
            last_match = m.end;
            for fmt in &*self.replacement {
                match *fmt {
                    FormatItem::Text(ref text) => {
                        buf.push_str(text);
                    }
                    FormatItem::Capture(i) => {
                        if let Some(cap) = cap.get_group(i) {
                            buf.extend(doc.byte_slice(cap.range()).chunks());
                        }
                    }
                    FormatItem::CaseChange(i, change) => {
                        if let Some(cap) = cap.get_group(i).filter(|i| !i.is_empty()) {
                            let mut chars = doc.byte_slice(cap.range()).chars();
                            match change {
                                CaseChange::Upcase => to_upper_case_with(chars, &mut buf),
                                CaseChange::Downcase => to_lower_case_with(chars, &mut buf),
                                CaseChange::Capitalize => {
                                    let first_char = chars.next().unwrap();
                                    buf.extend(first_char.to_uppercase());
                                    buf.extend(chars);
                                }
                                CaseChange::PascalCase => to_pascal_case_with(chars, &mut buf),
                                CaseChange::CamelCase => to_camel_case_with(chars, &mut buf),
                            }
                        }
                    }
                    FormatItem::Conditional(i, ref if_, ref else_) => {
                        if cap.get_group(i).is_none_or(|mat| mat.is_empty()) {
                            buf.push_str(else_)
                        } else {
                            buf.push_str(if_)
                        }
                    }
                }
            }
            if !self.global {
                break;
            }
        }
        buf.extend(doc.byte_slice(last_match..).chunks());
        buf
    }
}

impl TabstopIdx {
    fn elaborate(idx: usize) -> Self {
        TabstopIdx(idx.wrapping_sub(1))
    }
}
