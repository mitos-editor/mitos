use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::{icon_span, menu, panel, Markdown, Menu, Popup, PromptEvent};
use editor_core::chars;
use fuzzy::{MatchKind, Matcher};
use lsp_client::lsp;
use stdx::rope::RopeExt as _;
use theme::icons::ICONS;
use theme::{Color, Modifier, Style};
use tui::text::Line;
use tui::{buffer::Buffer as Surface, text::Span};
use view::features::completion::LspCompletionItem;
use view::features::completion::{
    CompletionItem, CompletionResponse, CompletionSession, ResolveHandler, SnippetCompletion,
    TextCompletion,
};
use view::features::signature_help::SignatureHelpInvoked;
use view::{graphics::Rect, Editor};

use std::cmp::Reverse;

#[derive(Clone, Copy)]
pub struct CompletionData {
    directory_style: Style,
    icons: bool,
}

impl menu::Item for CompletionItem {
    type Data = CompletionData;

    fn format(&self, data: &Self::Data) -> menu::Row<'_> {
        let deprecated = match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => {
                item.deprecated.unwrap_or_default()
                    || item
                        .tags
                        .as_ref()
                        .is_some_and(|tags| tags.contains(&lsp::CompletionItemTag::DEPRECATED))
            }
            CompletionItem::Path(_) | CompletionItem::Word(_) | CompletionItem::Snippet(_) => false,
        };

        let label = match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => item.label.as_str(),
            CompletionItem::Path(TextCompletion { label, .. })
            | CompletionItem::Word(TextCompletion { label, .. })
            | CompletionItem::Snippet(SnippetCompletion {
                text: TextCompletion { label, .. },
                ..
            }) => label,
        };

        let kind = match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => match item.kind {
                Some(lsp::CompletionItemKind::TEXT) => "text".into(),
                Some(lsp::CompletionItemKind::METHOD) => "method".into(),
                Some(lsp::CompletionItemKind::FUNCTION) => "function".into(),
                Some(lsp::CompletionItemKind::CONSTRUCTOR) => "constructor".into(),
                Some(lsp::CompletionItemKind::FIELD) => "field".into(),
                Some(lsp::CompletionItemKind::VARIABLE) => "variable".into(),
                Some(lsp::CompletionItemKind::CLASS) => "class".into(),
                Some(lsp::CompletionItemKind::INTERFACE) => "interface".into(),
                Some(lsp::CompletionItemKind::MODULE) => "module".into(),
                Some(lsp::CompletionItemKind::PROPERTY) => "property".into(),
                Some(lsp::CompletionItemKind::UNIT) => "unit".into(),
                Some(lsp::CompletionItemKind::VALUE) => "value".into(),
                Some(lsp::CompletionItemKind::ENUM) => "enum".into(),
                Some(lsp::CompletionItemKind::KEYWORD) => "keyword".into(),
                Some(lsp::CompletionItemKind::SNIPPET) => "snippet".into(),
                Some(lsp::CompletionItemKind::COLOR) => item
                    .documentation
                    .as_ref()
                    .and_then(|docs| {
                        let text = match docs {
                            lsp::Documentation::String(text) => text,
                            lsp::Documentation::MarkupContent(lsp::MarkupContent {
                                value, ..
                            }) => value,
                        };
                        // Language servers which send Color completion items tend to include a 6
                        // digit hex code at the end for the color. The extra 1 digit is for the '#'
                        text.get(text.len().checked_sub(7)?..)
                    })
                    .and_then(|c| Color::from_hex(c).ok())
                    .map_or("color".into(), |color| {
                        Line::from(vec![
                            Span::raw("color "),
                            Span::styled("■", Style::default().fg(color)),
                        ])
                    }),
                Some(lsp::CompletionItemKind::FILE) => "file".into(),
                Some(lsp::CompletionItemKind::REFERENCE) => "reference".into(),
                Some(lsp::CompletionItemKind::FOLDER) => "folder".into(),
                Some(lsp::CompletionItemKind::ENUM_MEMBER) => "enum_member".into(),
                Some(lsp::CompletionItemKind::CONSTANT) => "constant".into(),
                Some(lsp::CompletionItemKind::STRUCT) => "struct".into(),
                Some(lsp::CompletionItemKind::EVENT) => "event".into(),
                Some(lsp::CompletionItemKind::OPERATOR) => "operator".into(),
                Some(lsp::CompletionItemKind::TYPE_PARAMETER) => "type_param".into(),
                Some(kind) => {
                    log::error!("Received unknown completion item kind: {:?}", kind);
                    "".into()
                }
                None => "".into(),
            },
            CompletionItem::Path(TextCompletion { kind, .. })
            | CompletionItem::Word(TextCompletion { kind, .. })
            | CompletionItem::Snippet(SnippetCompletion {
                text: TextCompletion { kind, .. },
                ..
            }) => kind.as_ref().into(),
        };

        let kind_name = kind.spans.first().map_or("", |span| span.content.as_ref());
        let label = Span::styled(
            label,
            if deprecated {
                Style::default().add_modifier(Modifier::CROSSED_OUT)
            } else if kind_name == "folder" {
                data.directory_style
            } else {
                Style::default()
            },
        );

        if data.icons && kind_name != "color" {
            let icons = ICONS.load();
            if let Some(icon) = icons.kind().get(kind_name) {
                return menu::Row::new([
                    menu::Cell::from(icon_span(icon.with_padding(0, 1))),
                    menu::Cell::from(label),
                    menu::Cell::from(kind),
                ]);
            }
        }

        menu::Row::new([menu::Cell::from(label), menu::Cell::from(kind)])
    }
}

/// Wraps a Menu.
pub struct Completion {
    popup: Popup<Menu<CompletionItem>>,
    filter: String,
    matcher: Matcher,
    // Documentation resolution follows the highlighted menu item.
    resolve_handler: ResolveHandler,
}

impl Completion {
    pub const ID: &'static str = "completion";

    pub fn new(editor: &Editor, items: Vec<CompletionItem>, session: CompletionSession) -> Self {
        let preview_completion_insert = editor.config().completion.preview_insert;

        let data = CompletionData {
            directory_style: editor.theme.get("ui.text.directory"),
            icons: editor.config().icons,
        };

        // Then create the menu
        let menu = Menu::new(items, data, move |editor: &mut Editor, item, event| {
            match event {
                PromptEvent::Update if preview_completion_insert => {
                    if let Some(item) = item
                        && let Err(error) = editor.preview_completion(&session, item)
                    {
                        editor.set_error(|| error.to_string());
                        return;
                    }
                }
                PromptEvent::Validate => {
                    if let Some(item) = item
                        && let Err(error) = editor.accept_completion(&session, item)
                    {
                        editor.set_error(|| error.to_string());
                        return;
                    }
                }
                PromptEvent::Abort | PromptEvent::Update => {}
            }

            // In case the popup was deleted because of an intersection w/ the auto-complete menu.
            if event != PromptEvent::Update {
                editor
                    .handlers()
                    .trigger_signature_help(SignatureHelpInvoked::Automatic, editor);
            }
        });

        let popup = Popup::new(Self::ID, menu)
            .menu_style()
            .with_scrollbar(false)
            .ignore_escape_key(true);

        let (view, doc) = current_ref!(editor);
        let text = doc.text().slice(..);
        let cursor = doc.selection(view.id).primary().cursor(text);
        let offset = text
            .chars_at_char(cursor)
            .reversed()
            .take_while(|ch| chars::char_is_word(*ch))
            .count();
        let start_offset = cursor.saturating_sub(offset);

        let fragment = doc.text().slice_chars(start_offset..cursor);
        let mut completion = Self {
            popup,
            // TODO: expand nucleo api to allow moving straight to a Utf32String here
            // and avoid allocation during matching
            filter: String::from(fragment),
            matcher: Matcher::new(MatchKind::Completion),
            resolve_handler: ResolveHandler::new(),
        };

        // Score the initial typed fragment before displaying the menu.
        completion.score(false);

        completion
    }

    fn score(&mut self, incremental: bool) {
        let pattern = &self.filter;
        let pattern = self.matcher.query(pattern);
        let (matches, options) = self.popup.contents_mut().update_options();
        if incremental {
            matches.retain_mut(|(index, score)| {
                let option = &options[*index as usize];
                let text = option.filter_text();
                let new_score = self.matcher.score(&pattern, text);
                match new_score {
                    Some(new_score) => {
                        *score = new_score as u32 / 2;
                        true
                    }
                    None => false,
                }
            })
        } else {
            matches.clear();
            matches.extend(options.iter().enumerate().filter_map(|(i, option)| {
                let text = option.filter_text();
                self.matcher
                    .score(&pattern, text)
                    .map(|score| (i as u32, score as u32 / 3))
            }));
        }
        // Nucleo is meant as an FZF-like fuzzy matcher and only hides matches that are truly
        // impossible - as in the sequence of characters just doesn't appear. That doesn't work
        // well for completions with multiple language servers where all completions of the next
        // server are below the current one (so you would get good suggestions from the second
        // server below those of the first). Setting a reasonable cutoff below which to move bad
        // completions out of the way helps with that.
        //
        // The score computation is a heuristic derived from Nucleo internal constants that may
        // move upstream in the future. I want to test this out here to settle on a good number.
        let min_score = (7 + pattern.needle_len() as u32 * 14) / 3;
        matches.sort_unstable_by_key(|&(i, score)| {
            let option = &options[i as usize];
            (
                score <= min_score,
                Reverse(option.preselect()),
                option.provider_priority(),
                Reverse(score),
                i,
            )
        });
    }

    /// Appends (`c: Some(c)`) or removes (`c: None`) a character to/from the filter
    /// this should be called whenever the user types or deletes a character in insert mode.
    pub fn update_filter(&mut self, c: Option<char>) {
        // recompute menu based on matches
        let menu = self.popup.contents_mut();
        match c {
            Some(c) => self.filter.push(c),
            None => {
                self.filter.pop();
                if self.filter.is_empty() {
                    menu.clear();
                    return;
                }
            }
        }
        self.score(c.is_some());
        self.popup.contents_mut().reset_cursor();
    }

    pub fn replace_provider_completions(
        &mut self,
        response: &mut CompletionResponse,
        is_incomplete: bool,
    ) {
        let menu = self.popup.contents_mut();
        let (_, options) = menu.update_options();
        if is_incomplete {
            options.retain(|item| item.provider() != response.provider())
        }
        response.take_items(options);
        self.score(false);
        let menu = self.popup.contents_mut();
        menu.ensure_cursor_in_bounds();
    }

    pub fn is_empty(&self) -> bool {
        self.popup.contents().is_empty()
    }

    pub fn replace_item(
        &mut self,
        old_item: &impl PartialEq<CompletionItem>,
        new_item: CompletionItem,
    ) {
        self.popup.contents_mut().replace_option(old_item, new_item);
    }

    pub(crate) fn accept(&mut self, cx: &mut Context) -> EventResult {
        self.popup
            .contents_mut()
            .execute_action(crate::keymap::component::MenuAction::Accept, cx)
    }

    pub fn area(&mut self, viewport: Rect, editor: &Editor) -> Rect {
        self.popup.area(viewport, editor)
    }
}

impl Component for Completion {
    fn has_pending_keys(&self) -> bool {
        self.popup.has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        self.popup.handle_event(event, cx)
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.popup.required_size(viewport)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.popup.render(area, surface, cx);

        // if we have a selection, render a markdown popup on top/below with info
        let option = match self.popup.contents_mut().selection_mut() {
            Some(option) => option,
            None => return,
        };
        if let CompletionItem::Lsp(option) = option {
            self.resolve_handler.ensure_item_resolved(cx.editor, option);
        }
        // need to render:
        // option.detail
        // ---
        // option.documentation

        let Some(coords) = cx.editor.cursor().0 else {
            return;
        };
        let cursor_pos = coords.row as u16;
        let doc = doc!(cx.editor);
        let language = doc.language_name().unwrap_or("");

        let markdowned = |lang: &str, detail: Option<&str>, doc: Option<&str>| {
            let md = match (detail, doc) {
                (Some(detail), Some(doc)) => format!("```{lang}\n{detail}\n```\n{doc}"),
                (Some(detail), None) => format!("```{lang}\n{detail}\n```"),
                (None, Some(doc)) => doc.to_string(),
                (None, None) => String::new(),
            };
            Markdown::new(md, cx.editor.syn_loader.clone())
        };

        let mut markdown_doc = match option {
            CompletionItem::Lsp(option) => match &option.item.documentation {
                Some(lsp::Documentation::String(contents))
                | Some(lsp::Documentation::MarkupContent(lsp::MarkupContent {
                    kind: lsp::MarkupKind::PlainText,
                    value: contents,
                })) => {
                    // TODO: convert to wrapped text
                    markdowned(language, option.item.detail.as_deref(), Some(contents))
                }
                Some(lsp::Documentation::MarkupContent(lsp::MarkupContent {
                    kind: lsp::MarkupKind::Markdown,
                    value: contents,
                })) => {
                    // TODO: set language based on doc scope
                    markdowned(language, option.item.detail.as_deref(), Some(contents))
                }
                None if option.item.detail.is_some() => {
                    // TODO: set language based on doc scope
                    markdowned(language, option.item.detail.as_deref(), None)
                }
                None => return,
            },
            CompletionItem::Path(option)
            | CompletionItem::Word(option)
            | CompletionItem::Snippet(SnippetCompletion { text: option, .. }) => {
                let Some(doc) = option.documentation.as_deref() else {
                    return;
                };
                markdowned(language, None, Some(doc))
            }
        };

        let popup_area = self.popup.area(area, cx.editor);
        let doc_width_available = area.right().saturating_sub(popup_area.right());
        let doc_area = if doc_width_available > 30 {
            let mut doc_width = doc_width_available;
            let mut doc_height = area.bottom().saturating_sub(popup_area.top());
            let x = popup_area.right();
            let y = popup_area.top();

            if let Some((rel_width, rel_height)) =
                markdown_doc.required_size((doc_width, doc_height))
            {
                doc_width = rel_width.saturating_add(panel::BORDER_INSET).min(doc_width);
                doc_height = rel_height
                    .saturating_add(panel::BORDER_INSET)
                    .min(doc_height);
            }
            Rect::new(x, y, doc_width, doc_height)
        } else {
            // Documentation should not cover the cursor or the completion popup
            // Completion popup could be above or below the current line
            let avail_height_above = cursor_pos
                .min(popup_area.top())
                .saturating_sub(area.top())
                .saturating_sub(1);
            let avail_height_below = area
                .bottom()
                .saturating_sub(cursor_pos.max(popup_area.bottom()).saturating_add(1));
            let (y, avail_height) = if avail_height_below >= avail_height_above {
                (
                    area.bottom().saturating_sub(avail_height_below),
                    avail_height_below,
                )
            } else {
                (area.top(), avail_height_above)
            };
            if avail_height <= 1 {
                return;
            }

            Rect::new(area.x, y, area.width, avail_height.min(15))
        };

        // clear area
        let background = cx.editor.theme.get("ui.popup");
        surface.clear_with(doc_area, background);

        use tui::widgets::Widget;
        let block = panel::bordered(&cx.editor.theme, cx.editor.config().border_style);
        let markdown_area = block.inner(doc_area);
        Widget::render(block, doc_area, surface);

        markdown_doc.render(markdown_area, surface, cx);
    }
}
use tui::buffer::BufferExt as _;
