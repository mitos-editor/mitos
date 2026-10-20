//! Definition tags extracted from syntax queries without editor identities or UI state.
use crate::{
    syntax::{Loader, QueryMatchIterEvent},
    Syntax,
};
use editor_core::RopeSlice;
use std::iter;
use stdx::rope::{self, RopeSliceExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagKind {
    Class,
    Constant,
    Enum,
    Field,
    Function,
    Interface,
    Macro,
    Module,
    Section,
    Struct,
    Type,
}

impl TagKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Constant => "constant",
            Self::Enum => "enum",
            Self::Field => "field",
            Self::Function => "function",
            Self::Interface => "interface",
            Self::Macro => "macro",
            Self::Module => "module",
            Self::Section => "section",
            Self::Struct => "struct",
            Self::Type => "type",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "class" => Some(TagKind::Class),
            "constant" => Some(TagKind::Constant),
            "enum" => Some(TagKind::Enum),
            "field" => Some(TagKind::Field),
            "function" => Some(TagKind::Function),
            "interface" => Some(TagKind::Interface),
            "macro" => Some(TagKind::Macro),
            "module" => Some(TagKind::Module),
            "section" => Some(TagKind::Section),
            "struct" => Some(TagKind::Struct),
            "type" => Some(TagKind::Type),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub kind: TagKind,
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub start_line: usize,
    pub end_line: usize,
}

pub fn tags<'a>(
    syntax: &'a Syntax,
    loader: &'a Loader,
    text: RopeSlice<'a>,
    pattern: Option<&'a rope::Regex>,
) -> impl Iterator<Item = Tag> + 'a {
    let mut tags_iter = syntax.tags(text, loader, ..);

    iter::from_fn(move || loop {
        let QueryMatchIterEvent::Match(mat) = tags_iter.next()? else {
            continue;
        };
        let query = &loader
            .tag_query(tags_iter.current_language())
            .expect("must have a tags query to emit matches")
            .query;

        // Find the @definition.* and optional @name captures in this match.
        let mut def_capture = None::<(TagKind, std::ops::Range<u32>)>;
        let mut name_range = None::<std::ops::Range<u32>>;
        let name_capture = query.get_capture("name");

        for node in mat.nodes.iter() {
            let capture_name = query.capture_name(node.capture);
            if let Some(kind) = capture_name
                .strip_prefix("definition.")
                .and_then(TagKind::from_name)
            {
                def_capture = Some((kind, node.node.byte_range()));
            } else if name_capture == Some(node.capture) {
                name_range = Some(node.node.byte_range());
            }
        }

        let Some((kind, def_byte_range)) = def_capture else {
            continue;
        };
        let name_byte_range = name_range.unwrap_or_else(|| def_byte_range.clone());

        if pattern.is_some_and(|re| {
            !re.is_match(
                text.regex_input_at_bytes(
                    name_byte_range.start as usize..name_byte_range.end as usize,
                ),
            )
        }) {
            continue;
        }

        let name_start = text.byte_to_char(name_byte_range.start as usize);
        let name_end = text.byte_to_char(name_byte_range.end as usize);
        let def_start = text.byte_to_char(def_byte_range.start as usize);
        let def_end = text.byte_to_char(def_byte_range.end as usize);

        return Some(Tag {
            kind,
            name: text.slice(name_start..name_end).to_string(),
            start: def_start,
            end: def_end,
            start_line: text.char_to_line(def_start),
            end_line: text.char_to_line(def_end),
        });
    })
}
