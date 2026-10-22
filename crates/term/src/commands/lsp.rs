use super::{navigation::push_jump, Context};
use crate::ui::lsp::symbol_kind_scope;
use crate::{
    compositor::Compositor,
    job::Callback,
    ui::{self, overlay::overlaid, FileLocation, Picker, Popup, PromptEvent},
};
use editor_core::Selection;
use futures_util::FutureExt;
use language_config::LanguageServerFeature;
use lsp_client::{lsp, util::lsp_range_to_range, OffsetEncoding};
use std::path::Path;
use stdx::path;
use stdx::rope::RopeExt as _;
use theme::icons::ICONS;
use theme::Theme;
use tui::{
    text::{Line, Span},
    widgets::{Cell, Row},
};
use view::{
    action::Action as CodeActionItem,
    align_view,
    editor::Action,
    features::code_lenses::{self, CodeLensCommand},
    features::navigation::{self, Location, NavigationKind},
    features::signature_help::SignatureHelpInvoked,
    quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget},
    Align, Editor,
};
mod symbol_updates;

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
        .char_to_line_idx(selection.primary().cursor(doc.text().slice(..)));
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
                    editor.tree.focus() == view_id
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

#[derive(PartialEq)]
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
    Span::styled(
        display_symbol_kind(item.symbol.kind),
        data.theme.get("ui.text.inactive"),
    )
    .into()
}

fn symbol_kind_icon(
    item: &SymbolInformationItem,
    data: &SymbolPickerData,
) -> Option<Span<'static>> {
    if !data.show_icons {
        return None;
    }
    ICONS
        .load()
        .kind()
        .get(item.symbol.kind.as_str())
        .map(ui::icon_span)
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
    symbol: view::features::document_symbols::DocumentSymbol,
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
    let children = symbol.children;

    list.push(SymbolInformationItem {
        symbol: symbol.symbol,
        location: symbol.location,
        tree_prefix,
    });

    let last_child = children.len().saturating_sub(1);
    for (index, child) in children.into_iter().enumerate() {
        nested_symbol_to_flat(
            list,
            child,
            &child_ancestor_prefix,
            Some(index == last_child),
        );
    }
}

#[cfg(test)]
mod symbol_picker_tests {
    use super::*;
    use editor_core::Uri;
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
        children: impl IntoIterator<Item = view::features::document_symbols::DocumentSymbol>,
    ) -> view::features::document_symbols::DocumentSymbol {
        let mut item = symbol_information_item();
        item.symbol.name = name.into();
        view::features::document_symbols::DocumentSymbol {
            symbol: item.symbol,
            location: item.location,
            children: children.into_iter().collect(),
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
        let root = document_symbol(
            "root",
            [
                document_symbol("branch", [document_symbol("nested", [])]),
                document_symbol("leaf", []),
            ],
        );
        let mut symbols = Vec::new();

        nested_symbol_to_flat(&mut symbols, root, "", None);

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
    let (view, doc) = current_ref!(cx.editor);
    let (document, view) = (doc.id(), view.id);
    let task = match cx.editor.document_symbols(document, view) {
        Ok(task) => task,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };
    cx.jobs.callback(async move {
        let update = task.run().await;
        let call = move |editor: &mut Editor, compositor: &mut Compositor| {
            let Some(symbols) = update.and_then(|update| update.into_symbols(editor)) else {
                return;
            };
            let mut items = Vec::new();
            for symbol in symbols {
                nested_symbol_to_flat(&mut items, symbol, "", None);
            }
            let columns = [
                ui::PickerColumn::new("kind", symbol_kind_cell).with_prefix(symbol_kind_icon),
                // Nested document symbols include their fully expanded tree guide in
                // the name column. Legacy flat responses remain unprefixed.
                ui::PickerColumn::new("name", symbol_name_cell),
                ui::PickerColumn::new("container", symbol_container_cell),
            ];

            let picker = Picker::new(
                editor.redraw().clone(),
                columns,
                1, // name column
                items,
                picker_data,
                move |cx, item, action| {
                    jump_to_location(cx.editor, &item.location, action);
                },
            )
            .with_item_key(symbol_updates::symbol_key)
            .with_editor_updates(symbol_updates::document_updates(editor, document, view))
            .with_title("Document Symbols")
            .with_preview(move |_editor, item| location_to_file_location(&item.location))
            .with_quicklist(move |_editor, item| location_to_quicklist_entry(&item.location))
            .truncate_start(false);

            compositor.push(Box::new(overlaid(picker)))
        };

        Ok(Callback::EditorCompositor(Box::new(call)))
    });
}

struct WorkspaceSymbolPickerData {
    style: SymbolPickerData,
    symbols: std::sync::Arc<
        arc_swap::ArcSwap<view::features::workspace_symbols::WorkspaceSymbolsSession>,
    >,
}

pub fn workspace_symbol_picker(cx: &mut Context) {
    use crate::ui::picker::Injector;

    let document = doc!(cx.editor).id();
    let session = match cx.editor.workspace_symbols(document) {
        Ok(session) => session,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };
    let get_symbols = |pattern: &str,
                       editor: &mut Editor,
                       data: std::sync::Arc<WorkspaceSymbolPickerData>,
                       injector: &Injector<_, _>| {
        let request = data.symbols.load().query(editor, pattern.to_owned());
        let injector = injector.clone();
        async move {
            let mut task = request?;
            while let Some(response) = task.next_batch().await {
                match response {
                    Ok(items) => {
                        for item in items {
                            injector.push(SymbolInformationItem {
                                symbol: item.symbol,
                                location: item.location,
                                tree_prefix: String::new(),
                            })?;
                        }
                    }
                    Err(error) => log::error!("Error requesting workspace symbols: {error}"),
                }
            }
            Ok(())
        }
        .boxed()
    };
    let session = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(session));
    let columns = [
        ui::PickerColumn::new("kind", |item, data: &WorkspaceSymbolPickerData| {
            symbol_kind_cell(item, &data.style)
        })
        .with_prefix(|item, data| symbol_kind_icon(item, &data.style)),
        ui::PickerColumn::new("name", |item, data: &WorkspaceSymbolPickerData| {
            symbol_name_cell(item, &data.style)
        })
        .without_filtering(),
        ui::PickerColumn::new("container", |item, data: &WorkspaceSymbolPickerData| {
            symbol_container_cell(item, &data.style)
        }),
        ui::PickerColumn::new("path", |item, data: &WorkspaceSymbolPickerData| {
            symbol_path_cell(item, &data.style)
        }),
    ];

    let picker_data = WorkspaceSymbolPickerData {
        style: SymbolPickerData {
            show_icons: cx.editor.config().icons,
            theme: cx.editor.theme.clone(),
        },
        symbols: session.clone(),
    };

    let picker = Picker::new(
        cx.editor.redraw().clone(),
        columns,
        1, // name column
        [],
        picker_data,
        move |cx, item, action| {
            if !session.load().is_current(cx.editor) {
                return;
            }
            jump_to_location(cx.editor, &item.location, action);
        },
    )
    .with_preview(|_editor, item| location_to_file_location(&item.location))
    .with_quicklist(|_editor, item| location_to_quicklist_entry(&item.location))
    .with_title("Workspace Symbols")
    .with_dynamic_query(get_symbols, None, cx.jobs)
    .with_item_key(symbol_updates::symbol_key)
    .with_query_updates(symbol_updates::workspace_updates(cx.editor, document))
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
    let task = cx.editor.request_code_actions();
    cx.jobs.callback(async move {
        let update = task.run().await;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor, compositor| {
                let Some(actions) = update.and_then(|update| update.into_actions(editor)) else {
                    return;
                };
                if actions.is_empty() {
                    editor.set_error(|| "No code actions available");
                    return;
                }
                let mut picker = ui::Menu::new(actions, (), move |editor, action, event| {
                    if event == PromptEvent::Validate
                        && let Some(action) = action
                    {
                        action.execute(editor);
                    }
                });
                picker.move_down();
                let popup = Popup::new("code-action", picker)
                    .with_scrollbar(false)
                    .auto_close(true);
                compositor.replace_or_push("code-action", popup);
            },
        )))
    });
}

/// Precondition: `locations` should be non-empty.
pub(crate) fn goto_impl(
    title: &'static str,
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
            .with_preview(|_editor, location| location_to_file_location(location))
            .with_title(title);
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
    let (view, doc) = current_ref!(cx.editor);
    let task = match cx.editor.prepare_rename(doc.id(), view.id) {
        Ok(task) => task,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };
    let history_register = cx.register;
    cx.jobs.callback(async move {
        let update = task.run().await;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor, compositor| {
                let Some(result) = update.and_then(|update| update.into_result(editor)) else {
                    return;
                };
                let session = match result {
                    Ok(session) => session,
                    Err(error) => {
                        editor.set_error(|| error.to_string());
                        return;
                    }
                };
                let prefill = session.prefill().to_owned();
                let mut session = Some(session);
                let prompt = ui::Prompt::new(
                    "rename-to:".into(),
                    history_register,
                    ui::completers::none,
                    move |cx, input, event| {
                        if event == PromptEvent::Validate
                            && let Some(session) = session.take()
                            && let Err(error) = cx.editor.rename_symbol(session, input.to_owned())
                        {
                            cx.editor.set_error(|| error.to_string());
                        }
                    },
                )
                .with_line(prefill, editor);
                compositor.push(Box::new(prompt));
            },
        )))
    });
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
                        compositor.push(Box::new(overlaid(picker.with_title("LSP Commands"))))
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
