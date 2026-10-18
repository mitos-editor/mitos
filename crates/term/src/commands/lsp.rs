use super::{navigation::push_jump, Context};
use crate::ui::lsp::symbol_kind_scope;
use crate::{
    compositor::{self, Compositor},
    job::Callback,
    ui::{self, overlay::overlaid, FileLocation, Picker, Popup, PromptEvent},
};
use editor_core::{Selection, Uri};
use futures_util::{stream::FuturesUnordered, FutureExt};
use language_config::LanguageServerFeature;
use lsp_client::{
    block_on,
    lsp::{self, CodeAction, CodeActionOrCommand, CodeActionTriggerKind},
    util::lsp_range_to_range,
    LanguageServerId, OffsetEncoding,
};
use std::{collections::HashSet, path::Path};
use stdx::path;
use theme::icons::ICONS;
use theme::Theme;
use tokio_stream::StreamExt;
use tui::{
    text::{Line, Span},
    widgets::{Cell, Row},
};
use view::{
    action::{code_actions_for_range, Action as CodeActionItem, LspActionContext},
    align_view,
    editor::Action,
    features::code_lenses::{self, CodeLensCommand},
    features::navigation::{self, Location, NavigationKind},
    features::signature_help::SignatureHelpInvoked,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Align, Editor,
};

/// Gets the first language server that is attached to a document which supports a specific feature.
/// If there is no configured language server that supports the feature, this displays a status message.
/// Using this macro in a context where the editor automatically queries the LSP
/// (instead of when the user explicitly does so via a keybind like `gd`)
/// will spam the "No configured language server supports \<feature>" status message confusingly.
#[macro_export]
macro_rules! language_server_with_feature {
    ($editor:expr, $doc:expr, $feature:expr) => {{
        let language_server = $doc.language_servers_with_feature($feature).next();
        match language_server {
            Some(language_server) => language_server,
            None => {
                $editor
                    .set_error(|| format!("No configured language server supports {}", $feature));
                return;
            }
        }
    }};
}

impl ui::menu::Item for CodeLensCommand {
    type Data = ();

    fn format(&self, _: &Self::Data) -> Row<'_> {
        self.command.title.as_str().into()
    }
}

pub fn show_code_lenses_under_cursor(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    language_server_with_feature!(cx.editor, doc, LanguageServerFeature::CodeLens);
    let doc_id = doc.id();
    let view_id = view.id;
    let selection = doc.selection(view_id).clone();
    let line = doc
        .text()
        .char_to_line(selection.primary().cursor(doc.text().slice(..)));
    let Some(future) = code_lenses::resolve_code_lenses(cx.editor, doc_id, line) else {
        cx.editor.set_error(|| "No code lenses available");
        return;
    };
    cx.jobs.callback(async move {
        let resolved = future.await;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor, compositor| {
                let Some(mut resolved) = resolved else { return };
                let is_current = move |editor: &Editor| {
                    editor.tree.focus == view_id
                        && editor
                            .tree
                            .try_get(view_id)
                            .is_some_and(|view| view.document_id() == doc_id)
                        && editor
                            .document(doc_id)
                            .is_some_and(|doc| doc.selection(view_id) == &selection)
                };
                if !resolved.is_current(editor) || !is_current(editor) {
                    return;
                }
                if resolved.commands.is_empty() {
                    editor.set_error(|| "No code lenses available");
                    return;
                }
                resolved.cache(editor);
                let commands = std::mem::take(&mut resolved.commands);
                let mut menu = ui::Menu::new(commands, (), move |editor, lens, event| {
                    if event == PromptEvent::Validate
                        && resolved.is_current(editor)
                        && is_current(editor)
                        && let Some(lens) = lens
                    {
                        editor.execute_lsp_command(lens.command.clone(), lens.language_server_id);
                    }
                });
                menu.move_down();
                let popup = Popup::new("code-lens", menu)
                    .with_scrollbar(false)
                    .auto_close(true);
                compositor.replace_or_push("code-lens", popup);
            },
        )))
    });
}

struct SymbolInformationItem {
    location: Location,
    symbol: lsp::SymbolInformation,
    tree_prefix: String,
}

struct SymbolPickerData {
    show_icons: bool,
    theme: Theme,
}

fn location_to_file_location(location: &Location) -> Option<FileLocation<'_>> {
    let path = location.uri.as_path()?;
    let line = Some((
        location.range.start.line as usize,
        location.range.end.line as usize,
    ));
    Some((path.into(), line))
}

fn location_to_quicklist_entry(location: &Location) -> Option<QuicklistEntry> {
    let path = location.uri.as_path()?;
    Some(QuicklistEntry {
        target: QuicklistTarget::Path(path.to_path_buf()),
        position: QuicklistPosition::LspRange {
            range: location.range,
            offset_encoding: location.offset_encoding,
        },
    })
}

fn jump_to_location(editor: &mut Editor, location: &Location, action: Action) {
    let (view, doc) = current!(editor);
    push_jump(view, doc);

    let Some(path) = location.uri.as_path() else {
        editor.set_error(|| format!("unable to convert URI to filepath: {:?}", location.uri));
        return;
    };
    jump_to_position(
        editor,
        path,
        location.range,
        location.offset_encoding,
        action,
    );
}

pub(super) fn jump_to_position(
    editor: &mut Editor,
    path: &Path,
    range: lsp::Range,
    offset_encoding: OffsetEncoding,
    action: Action,
) {
    let doc = match editor.open(path, action) {
        Ok(id) => doc_mut!(editor, &id),
        Err(err) => {
            editor.set_error(|| format!("failed to open path: {:?}: {:?}", path, err));
            return;
        }
    };
    let view = view_mut!(editor);
    // TODO: convert inside server
    let new_range = if let Some(new_range) = lsp_range_to_range(doc.text(), range, offset_encoding)
    {
        new_range
    } else {
        log::warn!("lsp position out of bounds - {:?}", range);
        return;
    };
    // we flip the range so that the cursor sits on the start of the symbol
    // (for example start of the function).
    doc.set_selection(view.id, Selection::single(new_range.head, new_range.anchor));
    if action.align_view(view, doc.id()) {
        align_view(doc, view, Align::Center);
    }
}

fn display_symbol_kind(kind: lsp::SymbolKind) -> &'static str {
    match kind {
        lsp::SymbolKind::FILE => "file",
        lsp::SymbolKind::MODULE => "module",
        lsp::SymbolKind::NAMESPACE => "namespace",
        lsp::SymbolKind::PACKAGE => "package",
        lsp::SymbolKind::CLASS => "class",
        lsp::SymbolKind::METHOD => "method",
        lsp::SymbolKind::PROPERTY => "property",
        lsp::SymbolKind::FIELD => "field",
        lsp::SymbolKind::CONSTRUCTOR => "construct",
        lsp::SymbolKind::ENUM => "enum",
        lsp::SymbolKind::INTERFACE => "interface",
        lsp::SymbolKind::FUNCTION => "function",
        lsp::SymbolKind::VARIABLE => "variable",
        lsp::SymbolKind::CONSTANT => "constant",
        lsp::SymbolKind::STRING => "string",
        lsp::SymbolKind::NUMBER => "number",
        lsp::SymbolKind::BOOLEAN => "boolean",
        lsp::SymbolKind::ARRAY => "array",
        lsp::SymbolKind::OBJECT => "object",
        lsp::SymbolKind::KEY => "key",
        lsp::SymbolKind::NULL => "null",
        lsp::SymbolKind::ENUM_MEMBER => "enummem",
        lsp::SymbolKind::STRUCT => "struct",
        lsp::SymbolKind::EVENT => "event",
        lsp::SymbolKind::OPERATOR => "operator",
        lsp::SymbolKind::TYPE_PARAMETER => "typeparam",
        _ => {
            log::warn!("Unknown symbol kind: {:?}", kind);
            ""
        }
    }
}

fn symbol_kind_cell<'a>(item: &'a SymbolInformationItem, data: &'a SymbolPickerData) -> Cell<'a> {
    let mut spans = Vec::with_capacity(2);
    if data.show_icons {
        let icons = ICONS.load();
        if let Some(icon) = icons.kind().get(item.symbol.kind.as_str()) {
            spans.push(ui::icon_span(icon));
        }
    }
    spans.push(Span::styled(
        display_symbol_kind(item.symbol.kind),
        data.theme.get("ui.text.inactive"),
    ));
    Line::from(spans).into()
}

fn symbol_name_cell<'a>(item: &'a SymbolInformationItem, data: &'a SymbolPickerData) -> Cell<'a> {
    let mut spans = Vec::with_capacity(2);
    if !item.tree_prefix.is_empty() {
        spans.push(Span::styled(
            item.tree_prefix.as_str(),
            data.theme.get("ui.text.inactive"),
        ));
    }
    spans.push(Span::styled(
        item.symbol.name.as_str(),
        data.theme.get(symbol_kind_scope(item.symbol.kind)),
    ));
    Line::from(spans).into()
}

fn symbol_container_cell<'a>(
    item: &'a SymbolInformationItem,
    data: &'a SymbolPickerData,
) -> Cell<'a> {
    Span::styled(
        item.symbol.container_name.as_deref().unwrap_or_default(),
        data.theme.get("ui.text.inactive"),
    )
    .into()
}

fn symbol_path_cell<'a>(item: &'a SymbolInformationItem, data: &'a SymbolPickerData) -> Cell<'a> {
    let path = if let Some(path) = item.location.uri.as_path() {
        path::get_relative_path(path).to_string_lossy().to_string()
    } else {
        item.symbol.location.uri.to_string()
    };
    Span::styled(path, data.theme.get("ui.text.directory")).into()
}

fn nested_symbol_to_flat(
    list: &mut Vec<SymbolInformationItem>,
    file: &lsp::TextDocumentIdentifier,
    uri: &Uri,
    symbol: lsp::DocumentSymbol,
    offset_encoding: OffsetEncoding,
    ancestor_prefix: &str,
    is_last_child: Option<bool>,
) {
    let tree_prefix = match is_last_child {
        Some(true) => format!("{ancestor_prefix}└─ "),
        Some(false) => format!("{ancestor_prefix}├─ "),
        None => String::new(),
    };
    let child_ancestor_prefix = match is_last_child {
        Some(true) => format!("{ancestor_prefix}   "),
        Some(false) => format!("{ancestor_prefix}│  "),
        None => ancestor_prefix.to_owned(),
    };
    let children = symbol.children.unwrap_or_default();

    #[allow(deprecated)]
    list.push(SymbolInformationItem {
        symbol: lsp::SymbolInformation {
            name: symbol.name,
            kind: symbol.kind,
            tags: symbol.tags,
            deprecated: symbol.deprecated,
            location: lsp::Location::new(file.uri.clone(), symbol.selection_range),
            container_name: None,
        },
        location: Location {
            uri: uri.clone(),
            range: symbol.selection_range,
            offset_encoding,
        },
        tree_prefix,
    });

    let last_child = children.len().saturating_sub(1);
    for (index, child) in children.into_iter().enumerate() {
        nested_symbol_to_flat(
            list,
            file,
            uri,
            child,
            offset_encoding,
            &child_ancestor_prefix,
            Some(index == last_child),
        );
    }
}

#[cfg(test)]
mod symbol_picker_tests {
    use super::*;
    use theme::DEFAULT_THEME;

    fn symbol_information_item() -> SymbolInformationItem {
        let uri = lsp::Url::parse("file:///tmp/main.rs").unwrap();
        #[allow(deprecated)]
        let symbol = lsp::SymbolInformation {
            name: "main".into(),
            kind: lsp::SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            location: lsp::Location::new(uri.clone(), lsp::Range::default()),
            container_name: Some("crate".into()),
        };
        SymbolInformationItem {
            location: Location {
                uri: Uri::try_from(&uri).unwrap(),
                range: lsp::Range::default(),
                offset_encoding: OffsetEncoding::Utf16,
            },
            symbol,
            tree_prefix: String::new(),
        }
    }

    fn document_symbol(
        name: &str,
        children: impl IntoIterator<Item = lsp::DocumentSymbol>,
    ) -> lsp::DocumentSymbol {
        let children = children.into_iter().collect::<Vec<_>>();
        #[allow(deprecated)]
        lsp::DocumentSymbol {
            name: name.into(),
            detail: None,
            kind: lsp::SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            range: lsp::Range::default(),
            selection_range: lsp::Range::default(),
            children: (!children.is_empty()).then_some(children),
        }
    }

    #[test]
    fn cells_use_semantic_theme_styles() {
        let item = symbol_information_item();
        let data = SymbolPickerData {
            show_icons: false,
            theme: DEFAULT_THEME.clone(),
        };

        let kind = symbol_kind_cell(&item, &data);
        let name = symbol_name_cell(&item, &data);
        let container = symbol_container_cell(&item, &data);
        let path = symbol_path_cell(&item, &data);

        assert_eq!(
            kind.content.lines[0].spans[0].style,
            DEFAULT_THEME.get("ui.text.inactive").into()
        );
        assert_eq!(
            name.content.lines[0].spans[0].style,
            DEFAULT_THEME.get("function").into()
        );
        assert_eq!(
            container.content.lines[0].spans[0].style,
            DEFAULT_THEME.get("ui.text.inactive").into()
        );
        assert_eq!(
            path.content.lines[0].spans[0].style,
            DEFAULT_THEME.get("ui.text.directory").into()
        );
    }

    #[test]
    fn nested_symbols_are_fully_expanded_with_tree_prefixes() {
        let uri = lsp::Url::parse("file:///tmp/main.rs").unwrap();
        let file = lsp::TextDocumentIdentifier::new(uri.clone());
        let root = document_symbol(
            "root",
            [
                document_symbol("branch", [document_symbol("nested", [])]),
                document_symbol("leaf", []),
            ],
        );
        let mut symbols = Vec::new();

        nested_symbol_to_flat(
            &mut symbols,
            &file,
            &Uri::try_from(&uri).unwrap(),
            root,
            OffsetEncoding::Utf16,
            "",
            None,
        );

        assert_eq!(
            symbols
                .iter()
                .map(|item| (item.tree_prefix.as_str(), item.symbol.name.as_str()))
                .collect::<Vec<_>>(),
            [
                ("", "root"),
                ("├─ ", "branch"),
                ("│  └─ ", "nested"),
                ("└─ ", "leaf"),
            ]
        );
    }
}

pub fn symbol_picker(cx: &mut Context) {
    let picker_data = SymbolPickerData {
        show_icons: cx.editor.config().icons,
        theme: cx.editor.theme.clone(),
    };
    let doc = doc!(cx.editor);

    let mut seen_language_servers = HashSet::new();

    let mut futures: FuturesUnordered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
        .filter(|ls| seen_language_servers.insert(ls.id()))
        .map(|language_server| {
            let request = language_server.document_symbols(doc.identifier()).unwrap();
            let offset_encoding = language_server.offset_encoding();
            let doc_id = doc.identifier();
            let doc_uri = doc
                .uri()
                .expect("docs with active language servers must be backed by paths");

            async move {
                let symbols = match request.await? {
                    Some(symbols) => symbols,
                    None => return anyhow::Ok(vec![]),
                };
                // lsp has two ways to represent symbols (flat/nested)
                // convert the nested variant to flat, so that we have a homogeneous list
                let symbols = match symbols {
                    lsp::DocumentSymbolResponse::Flat(symbols) => symbols
                        .into_iter()
                        .map(|symbol| SymbolInformationItem {
                            location: Location {
                                uri: doc_uri.clone(),
                                range: symbol.location.range,
                                offset_encoding,
                            },
                            symbol,
                            tree_prefix: String::new(),
                        })
                        .collect(),
                    lsp::DocumentSymbolResponse::Nested(symbols) => {
                        let mut flat_symbols = Vec::new();
                        for symbol in symbols {
                            nested_symbol_to_flat(
                                &mut flat_symbols,
                                &doc_id,
                                &doc_uri,
                                symbol,
                                offset_encoding,
                                "",
                                None,
                            )
                        }
                        flat_symbols
                    }
                };
                Ok(symbols)
            }
        })
        .collect();

    if futures.is_empty() {
        cx.editor
            .set_error(|| "No configured language server supports document symbols");
        return;
    }

    cx.jobs.callback(async move {
        let mut symbols = Vec::new();
        while let Some(response) = futures.next().await {
            match response {
                Ok(mut items) => symbols.append(&mut items),
                Err(err) => log::error!("Error requesting document symbols: {err}"),
            }
        }
        let call = move |editor: &mut Editor, compositor: &mut Compositor| {
            let columns = [
                ui::PickerColumn::new("kind", symbol_kind_cell),
                // Nested document symbols include their fully expanded tree guide in
                // the name column. Legacy flat responses remain unprefixed.
                ui::PickerColumn::new("name", symbol_name_cell),
                ui::PickerColumn::new("container", symbol_container_cell),
            ];

            let picker = Picker::new(
                editor.redraw().clone(),
                columns,
                1, // name column
                symbols,
                picker_data,
                move |cx, item, action| {
                    jump_to_location(cx.editor, &item.location, action);
                },
            )
            .with_preview(move |_editor, item| location_to_file_location(&item.location))
            .with_quicklist(move |_editor, item| location_to_quicklist_entry(&item.location))
            .truncate_start(false);

            compositor.push(Box::new(overlaid(picker)))
        };

        Ok(Callback::EditorCompositor(Box::new(call)))
    });
}

pub fn workspace_symbol_picker(cx: &mut Context) {
    use crate::ui::picker::Injector;

    let doc = doc!(cx.editor);
    if doc
        .language_servers_with_feature(LanguageServerFeature::WorkspaceSymbols)
        .count()
        == 0
    {
        cx.editor
            .set_error(|| "No configured language server supports workspace symbols");
        return;
    }

    let get_symbols = |pattern: &str, editor: &mut Editor, _data, injector: &Injector<_, _>| {
        let doc = doc!(editor);
        let mut seen_language_servers = HashSet::new();
        let mut futures: FuturesUnordered<_> = doc
            .language_servers_with_feature(LanguageServerFeature::WorkspaceSymbols)
            .filter(|ls| seen_language_servers.insert(ls.id()))
            .map(|language_server| {
                let request = language_server
                    .workspace_symbols(pattern.to_string())
                    .unwrap();
                let offset_encoding = language_server.offset_encoding();
                async move {
                    let symbols = request
                        .await?
                        .and_then(|resp| match resp {
                            lsp::WorkspaceSymbolResponse::Flat(symbols) => Some(symbols),
                            lsp::WorkspaceSymbolResponse::Nested(_) => None,
                        })
                        .unwrap_or_default();

                    let response: Vec<_> = symbols
                        .into_iter()
                        .filter_map(|symbol| {
                            let uri = match Uri::try_from(&symbol.location.uri) {
                                Ok(uri) => uri,
                                Err(err) => {
                                    log::warn!("discarding symbol with invalid URI: {err}");
                                    return None;
                                }
                            };
                            Some(SymbolInformationItem {
                                location: Location {
                                    uri,
                                    range: symbol.location.range,
                                    offset_encoding,
                                },
                                symbol,
                                tree_prefix: String::new(),
                            })
                        })
                        .collect();

                    anyhow::Ok(response)
                }
            })
            .collect();

        if futures.is_empty() {
            editor.set_error(|| "No configured language server supports workspace symbols");
        }

        let injector = injector.clone();
        async move {
            while let Some(response) = futures.next().await {
                match response {
                    Ok(items) => {
                        for item in items {
                            injector.push(item)?;
                        }
                    }
                    Err(err) => log::error!("Error requesting workspace symbols: {err}"),
                }
            }
            Ok(())
        }
        .boxed()
    };
    let columns = [
        ui::PickerColumn::new("kind", symbol_kind_cell),
        ui::PickerColumn::new("name", symbol_name_cell).without_filtering(),
        ui::PickerColumn::new("container", symbol_container_cell),
        ui::PickerColumn::new("path", symbol_path_cell),
    ];

    let picker_data = SymbolPickerData {
        show_icons: cx.editor.config().icons,
        theme: cx.editor.theme.clone(),
    };

    let picker = Picker::new(
        cx.editor.redraw().clone(),
        columns,
        1, // name column
        [],
        picker_data,
        move |cx, item, action| {
            jump_to_location(cx.editor, &item.location, action);
        },
    )
    .with_preview(|_editor, item| location_to_file_location(&item.location))
    .with_quicklist(|_editor, item| location_to_quicklist_entry(&item.location))
    .with_dynamic_query(get_symbols, None, cx.jobs)
    .truncate_start(false);

    cx.push_layer(Box::new(overlaid(picker)));
}

impl ui::menu::Item for CodeActionItem {
    type Data = ();
    fn format(&self, _data: &Self::Data) -> Row<'_> {
        Row::new([self.title()])
    }
}

pub fn code_action(cx: &mut Context) {
    let mode = cx.editor.mode();
    let (view, doc) = current!(cx.editor);

    let doc_id = doc.id();
    let view_id = view.id;
    let version = doc.version();
    let selection = doc.selection(view_id).clone();

    let action_context = LspActionContext::new(doc);
    let mut futures: FuturesUnordered<_> = code_actions_for_range(
        doc,
        selection.primary(),
        None,
        CodeActionTriggerKind::INVOKED,
    )
    .into_iter()
    .map(|(request, ls_id)| {
        let action_context = action_context.clone();
        async move {
            let Some(mut actions) = request.await? else {
                return anyhow::Ok(Vec::new());
            };

            // remove disabled code actions
            actions.retain(|action| {
                matches!(
                    action,
                    CodeActionOrCommand::Command(_)
                        | CodeActionOrCommand::CodeAction(CodeAction { disabled: None, .. })
                )
            });

            Ok(actions
                .into_iter()
                .map(|lsp_item| CodeActionItem::lsp(action_context.clone(), ls_id, lsp_item))
                .collect())
        }
        .boxed()
    })
    .collect();

    futures.push(cx.editor.spelling_actions().boxed());

    cx.jobs.callback(async move {
        let mut actions = Vec::new();

        while let Some(output) = futures.next().await {
            match output {
                Ok(mut items) => actions.append(&mut items),
                Err(err) => log::error!("while gathering code actions: {err}"),
            }
        }

        // Sort the gathered actions into a useful order, highest priority first. See
        // `lsp_code_action_priority` for how LSP actions are ranked.
        actions.sort_by_key(|action| std::cmp::Reverse(action.priority));

        let call = move |editor: &mut Editor, compositor: &mut Compositor| {
            let (view, doc) = current_ref!(editor);
            // Input may have changed the target while the providers were computing actions.
            if view.id != view_id
                || doc.id() != doc_id
                || doc.version() != version
                || editor.mode() != mode
                || doc.selection(view_id) != &selection
            {
                return;
            }
            if actions.is_empty() {
                editor.set_error(|| "No code actions available");
                return;
            }
            let mut picker = ui::Menu::new(actions, (), move |editor, action, event| {
                if event != PromptEvent::Validate {
                    return;
                }
                // Always present on validate.
                action.unwrap().execute(editor);
            });
            picker.move_down(); // pre-select the first item

            let popup = Popup::new("code-action", picker)
                .with_scrollbar(false)
                .auto_close(true);

            compositor.replace_or_push("code-action", popup);
        };

        Ok(Callback::EditorCompositor(Box::new(call)))
    });
}

/// Precondition: `locations` should be non-empty.
pub(crate) fn goto_impl(
    editor: &mut Editor,
    compositor: &mut Compositor,
    locations: Vec<Location>,
) {
    let cwdir = stdx::env::current_working_dir();

    match locations.as_slice() {
        [location] => {
            jump_to_location(editor, location, Action::Replace);
        }
        [] => unreachable!("`locations` should be non-empty for `goto_impl`"),
        _locations => {
            let columns = [ui::PickerColumn::new(
                "location",
                |item: &Location, cwdir: &std::path::PathBuf| {
                    let path = if let Some(path) = item.uri.as_path() {
                        path.strip_prefix(cwdir).unwrap_or(path).to_string_lossy()
                    } else {
                        item.uri.to_string().into()
                    };

                    format!("{path}:{}", item.range.start.line + 1).into()
                },
            )];

            let picker = Picker::new(
                editor.redraw().clone(),
                columns,
                0,
                locations,
                cwdir,
                |cx, location, action| jump_to_location(cx.editor, location, action),
            )
            .with_preview(|_editor, location| location_to_file_location(location));
            compositor.push(Box::new(overlaid(picker)));
        }
    }
}

pub fn goto_declaration(cx: &mut Context) {
    navigation::request(cx.editor, NavigationKind::Declaration);
}

pub fn goto_definition(cx: &mut Context) {
    navigation::request(cx.editor, NavigationKind::Definition);
}

pub fn goto_type_definition(cx: &mut Context) {
    navigation::request(cx.editor, NavigationKind::TypeDefinition);
}

pub fn goto_implementation(cx: &mut Context) {
    navigation::request(cx.editor, NavigationKind::Implementation);
}

pub fn goto_reference(cx: &mut Context) {
    navigation::request(cx.editor, NavigationKind::References);
}

pub fn signature_help(cx: &mut Context) {
    cx.editor
        .handlers()
        .trigger_signature_help(SignatureHelpInvoked::Manual, cx.editor)
}

pub fn hover(cx: &mut Context) {
    view::features::hover::request(cx.editor);
}

pub fn rename_symbol(cx: &mut Context) {
    fn get_prefill_from_word_boundary(editor: &Editor) -> String {
        let (view, doc) = current_ref!(editor);
        let text = doc.text().slice(..);
        let primary_selection = doc.selection(view.id).primary();
        if primary_selection.len() > 1 {
            primary_selection
        } else {
            use editor_core::textobject::{textobject_word, TextObject};
            textobject_word(text, primary_selection, TextObject::Inside, 1, false)
        }
        .fragment(text)
        .into()
    }

    fn get_prefill_from_lsp_response(
        editor: &Editor,
        offset_encoding: OffsetEncoding,
        response: Option<lsp::PrepareRenameResponse>,
    ) -> Result<String, &'static str> {
        match response {
            Some(lsp::PrepareRenameResponse::Range(range)) => {
                let text = doc!(editor).text();

                Ok(lsp_range_to_range(text, range, offset_encoding)
                    .ok_or("lsp sent invalid selection range for rename")?
                    .fragment(text.slice(..))
                    .into())
            }
            Some(lsp::PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) => {
                Ok(placeholder)
            }
            Some(lsp::PrepareRenameResponse::DefaultBehavior { .. }) => {
                Ok(get_prefill_from_word_boundary(editor))
            }
            None => Err("lsp did not respond to prepare rename request"),
        }
    }

    fn create_rename_prompt(
        editor: &Editor,
        prefill: String,
        history_register: Option<char>,
        language_server_id: Option<LanguageServerId>,
    ) -> Box<ui::Prompt> {
        let prompt = ui::Prompt::new(
            "rename-to:".into(),
            history_register,
            ui::completers::none,
            move |cx: &mut compositor::Context, input: &str, event: PromptEvent| {
                if event != PromptEvent::Validate {
                    return;
                }
                let (view, doc) = current!(cx.editor);

                let Some(language_server) = doc
                    .language_servers_with_feature(LanguageServerFeature::RenameSymbol)
                    .find(|ls| language_server_id.is_none_or(|id| id == ls.id()))
                else {
                    cx.editor
                        .set_error(|| "No configured language server supports symbol renaming");
                    return;
                };

                let offset_encoding = language_server.offset_encoding();
                let pos = doc.position(view.id, offset_encoding);
                let future = language_server
                    .rename_symbol(doc.identifier(), pos, input.to_string())
                    .unwrap();

                match block_on(future) {
                    Ok(edits) => {
                        let _ = cx
                            .editor
                            .apply_workspace_edit(offset_encoding, &edits.unwrap_or_default());
                    }
                    Err(err) => cx.editor.set_error(|| err.to_string()),
                }
            },
        )
        .with_line(prefill, editor);

        Box::new(prompt)
    }

    let (view, doc) = current_ref!(cx.editor);
    let history_register = cx.register;

    if doc
        .language_servers_with_feature(LanguageServerFeature::RenameSymbol)
        .next()
        .is_none()
    {
        cx.editor
            .set_error(|| "No configured language server supports symbol renaming");
        return;
    }

    let language_server_with_prepare_rename_support = doc
        .language_servers_with_feature(LanguageServerFeature::RenameSymbol)
        .find(|ls| {
            matches!(
                ls.capabilities().rename_provider,
                Some(lsp::OneOf::Right(lsp::RenameOptions {
                    prepare_provider: Some(true),
                    ..
                }))
            )
        });

    if let Some(language_server) = language_server_with_prepare_rename_support {
        let ls_id = language_server.id();
        let offset_encoding = language_server.offset_encoding();
        let pos = doc.position(view.id, offset_encoding);
        let future = language_server
            .prepare_rename(doc.identifier(), pos)
            .unwrap();
        cx.callback(
            future,
            move |editor, compositor, response: Option<lsp::PrepareRenameResponse>| {
                let prefill = match get_prefill_from_lsp_response(editor, offset_encoding, response)
                {
                    Ok(p) => p,
                    Err(e) => {
                        editor.set_error(|| e);
                        return;
                    }
                };

                let prompt = create_rename_prompt(editor, prefill, history_register, Some(ls_id));

                compositor.push(prompt);
            },
        );
    } else {
        let prefill = get_prefill_from_word_boundary(cx.editor);
        let prompt = create_rename_prompt(cx.editor, prefill, history_register, None);
        cx.push_layer(prompt);
    }
}

pub fn select_references_to_symbol_under_cursor(cx: &mut Context) {
    navigation::request(cx.editor, NavigationKind::SelectReferences);
}

pub(super) mod typed {
    //! Typable lsp commands.

    use crate::{
        compositor::{self, Compositor},
        job::{self, Callback},
        ui::{self, overlay::overlaid, PromptEvent},
    };
    use ::command_line::Args;
    use anyhow::anyhow;
    use language_config::LanguageServerFeature;
    use serde_json::Value;
    use view::Editor;

    #[cold]
    pub(in crate::commands) fn lsp_workspace_command(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let doc = doc!(cx.editor);
        let ls_id_commands = doc
            .language_servers_with_feature(LanguageServerFeature::WorkspaceCommand)
            .flat_map(|ls| {
                ls.capabilities()
                    .execute_command_provider
                    .iter()
                    .flat_map(|options| options.commands.iter())
                    .map(|command| (ls.id(), command))
            });

        if args.is_empty() {
            let commands = ls_id_commands
                .map(|(ls_id, command)| {
                    (
                        ls_id,
                        lsp_client::lsp::Command {
                            title: command.clone(),
                            command: command.clone(),
                            arguments: None,
                        },
                    )
                })
                .collect::<Vec<_>>();
            let callback = async move {
                let call: job::Callback = Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor| {
                        let columns = [ui::PickerColumn::new(
                            "title",
                            |(_ls_id, command): &(_, lsp_client::lsp::Command), _| {
                                command.title.as_str().into()
                            },
                        )];
                        let picker = ui::Picker::new(
                            editor.redraw().clone(),
                            columns,
                            0,
                            commands,
                            (),
                            move |cx, (ls_id, command), _action| {
                                cx.editor.execute_lsp_command(command.clone(), *ls_id);
                            },
                        );
                        compositor.push(Box::new(overlaid(picker)))
                    },
                ));
                Ok(call)
            };
            cx.jobs.callback(callback);
        } else {
            let command = args[0].to_string();
            let matches: Vec<_> = ls_id_commands
                .filter(|(_ls_id, c)| *c == &command)
                .collect();

            match matches.as_slice() {
                [(ls_id, _command)] => {
                    let arguments = args
                        .get(1)
                        .map(|rest| {
                            serde_json::Deserializer::from_str(rest)
                                .into_iter()
                                .collect::<Result<Vec<Value>, _>>()
                                .map_err(|err| anyhow!("failed to parse arguments: {err}"))
                        })
                        .transpose()?
                        .filter(|args| !args.is_empty());

                    cx.editor.execute_lsp_command(
                        lsp_client::lsp::Command {
                            title: command.clone(),
                            arguments,
                            command,
                        },
                        *ls_id,
                    );
                }
                [] => {
                    cx.editor.set_status(format!(
                        "`{command}` is not supported for any language server"
                    ));
                }
                _ => {
                    cx.editor.set_status(format!(
                        "`{command}` supported by multiple language servers"
                    ));
                }
            }
        }
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn lsp_restart(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let document = doc!(cx.editor).id();
        let servers: Vec<_> = args.iter().map(|arg| arg.as_ref()).collect();
        cx.editor.restart_language_servers(document, &servers)
    }

    #[cold]
    pub(in crate::commands) fn lsp_stop(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }
        let document = doc!(cx.editor).id();
        let servers: Vec<_> = args.iter().map(|arg| arg.as_ref()).collect();
        cx.editor.stop_language_servers(document, &servers)
    }
}
