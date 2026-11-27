//! Expandable hierarchy trees using the regular picker preview and navigation.

use super::*;
use crate::{
    keymap::component::PickerItemAction,
    ui::picker::{PickerCallbackResult, UpdateContext},
};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use view::features::hierarchy::{
    HierarchyDirection as Direction, HierarchyItem, HierarchyKind, HierarchySession,
};

pub fn incoming_calls(cx: &mut Context) {
    open(cx, HierarchyPicker::IncomingCalls);
}

pub fn outgoing_calls(cx: &mut Context) {
    open(cx, HierarchyPicker::OutgoingCalls);
}

pub fn type_hierarchy(cx: &mut Context) {
    open(cx, HierarchyPicker::Types);
}

#[derive(Clone, Copy)]
enum HierarchyPicker {
    IncomingCalls,
    OutgoingCalls,
    Types,
}

impl HierarchyPicker {
    fn kind(self) -> HierarchyKind {
        match self {
            Self::IncomingCalls | Self::OutgoingCalls => HierarchyKind::Call,
            Self::Types => HierarchyKind::Type,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::IncomingCalls => "Incoming Calls",
            Self::OutgoingCalls => "Outgoing Calls",
            Self::Types => "Type Hierarchy",
        }
    }
}

fn open(cx: &mut Context, hierarchy: HierarchyPicker) {
    let (view, doc) = current_ref!(cx.editor);
    let task = match cx.editor.hierarchy(doc.id(), view.id, hierarchy.kind()) {
        Ok(task) => task,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };
    cx.jobs.callback(async move {
        let update = task.run().await;
        Ok(Callback::ui(move |compositor, cx| {
            let Some((session, roots)) = update.and_then(|update| update.resolve(cx.editor)) else {
                return;
            };
            if roots.is_empty() {
                cx.editor.set_error(|| "No hierarchy found");
                return;
            }
            compositor.push(Box::new(overlaid(picker(
                cx.editor, session, roots, hierarchy,
            ))));
        }))
    });
}

struct Node {
    item: Arc<HierarchyItem>,
    parent: Option<usize>,
    direction: Option<Direction>,
    section: bool,
    children: Option<Vec<usize>>,
    recursive: bool,
    failed: bool,
}

struct Tree {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    expanded: ui_core::tree::Tree<usize>,
    loading: Option<usize>,
    query: String,
    stale: bool,
    theme: theme::Theme,
}

#[derive(Clone, PartialEq)]
struct Row {
    id: usize,
    item: Arc<HierarchyItem>,
    depth: usize,
    label: String,
    expanded: bool,
    leaf: bool,
    section: bool,
    matches: bool,
}

type Data = Arc<Mutex<Tree>>;

impl Tree {
    fn new(roots: Vec<HierarchyItem>, hierarchy: HierarchyPicker, theme: theme::Theme) -> Self {
        let mut tree = Self {
            nodes: Vec::new(),
            roots: Vec::new(),
            expanded: Default::default(),
            loading: None,
            query: String::new(),
            stale: false,
            theme,
        };
        let call_direction = match hierarchy {
            HierarchyPicker::IncomingCalls => Some(Direction::Incoming),
            HierarchyPicker::OutgoingCalls => Some(Direction::Outgoing),
            HierarchyPicker::Types => None,
        };
        for item in roots {
            let root = tree.nodes.len();
            let item = Arc::new(item);
            tree.roots.push(root);
            tree.nodes.push(Node {
                item: item.clone(),
                parent: None,
                direction: call_direction,
                section: false,
                children: call_direction.is_none().then(|| vec![root + 1, root + 2]),
                recursive: false,
                failed: false,
            });
            tree.expanded.toggle(root);
            if call_direction.is_some() {
                continue;
            }
            for direction in [Direction::Supertypes, Direction::Subtypes] {
                tree.nodes.push(Node {
                    item: item.clone(),
                    parent: Some(root),
                    direction: Some(direction),
                    section: true,
                    children: None,
                    recursive: false,
                    failed: false,
                });
            }
        }
        tree
    }

    fn rows(&self) -> Vec<Row> {
        if self.stale {
            return Vec::new();
        }
        let mut expanded = self.expanded.clone();
        if !self.query.is_empty() {
            // Search already loaded branches, retaining the ancestor chain.
            for id in 0..self.nodes.len() {
                if !expanded.is_expanded(&id) {
                    expanded.toggle(id);
                }
            }
        }
        let rows = expanded
            .rows(
                self.roots.clone(),
                |id| id,
                |&id| Ok::<_, ()>(self.nodes[id].children.clone().unwrap_or_default()),
            )
            .unwrap();
        let mut matcher = None;
        let query = nucleo::pattern::Pattern::parse(
            &self.query,
            nucleo::pattern::CaseMatching::Smart,
            nucleo::pattern::Normalization::Smart,
        );
        let mut scratch = Vec::new();
        let mut matches = vec![false; self.nodes.len()];
        ui_core::tree::filter_rows(rows, |&id| {
            matches[id] = self.query.is_empty()
                || query
                    .score(
                        nucleo::Utf32Str::new(&self.label(id), &mut scratch),
                        matcher
                            .get_or_insert_with(|| nucleo::Matcher::new(nucleo::Config::DEFAULT)),
                    )
                    .is_some();
            matches[id]
        })
        .into_iter()
        .map(|(id, depth)| {
            let node = &self.nodes[id];
            Row {
                id,
                item: node.item.clone(),
                depth,
                label: self.label(id),
                expanded: self.expanded.is_expanded(&id),
                leaf: node.recursive || node.children.as_ref().is_some_and(Vec::is_empty),
                section: node.section,
                matches: matches[id],
            }
        })
        .collect()
    }

    fn label(&self, id: usize) -> String {
        let node = &self.nodes[id];
        let name = if node.section {
            node.direction.unwrap().title()
        } else {
            &node.item.name
        };
        let suffix = if self.loading == Some(id) {
            " (loading…)"
        } else if node.failed {
            " (failed; collapse and expand to retry)"
        } else if node.recursive {
            " (recursive)"
        } else if node.children.as_ref().is_some_and(Vec::is_empty) {
            " (none)"
        } else {
            ""
        };
        format!("{name}{suffix}")
    }

    fn toggle(&mut self, id: usize, action: PickerItemAction) {
        let node = &mut self.nodes[id];
        if node.recursive || node.children.as_ref().is_some_and(Vec::is_empty) {
            return;
        }
        let expanded = self.expanded.is_expanded(&id);
        if matches!(action, PickerItemAction::Expand) && expanded
            || matches!(action, PickerItemAction::Collapse) && !expanded
        {
            return;
        }
        self.expanded.toggle(id);
        if !expanded {
            node.failed = false;
        }
    }

    fn set_children(&mut self, id: usize, children: Vec<HierarchyItem>) {
        let mut ids = Vec::new();
        for item in children {
            let mut parent = Some(id);
            let mut recursive = false;
            while let Some(ancestor) = parent {
                let node = &self.nodes[ancestor];
                recursive |= node.item.same_symbol(&item);
                parent = node.parent;
            }
            ids.push(self.nodes.len());
            self.nodes.push(Node {
                item: Arc::new(item),
                parent: Some(id),
                direction: self.nodes[id].direction,
                section: false,
                children: None,
                recursive,
                failed: false,
            });
        }
        self.nodes[id].children = Some(ids);
    }
}

fn picker(
    editor: &Editor,
    session: HierarchySession,
    roots: Vec<HierarchyItem>,
    hierarchy: HierarchyPicker,
) -> Picker<Row, Data> {
    let data = Arc::new(Mutex::new(Tree::new(
        roots,
        hierarchy,
        editor.theme.clone(),
    )));
    let rows = data.lock().unwrap().rows();
    let accept_session = session.clone();
    let toggle_session = session.clone();
    let updates = updates(session, data.clone());
    Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            ui::PickerColumn::new("symbol", |row: &Row, data: &Data| {
                let marker = if row.leaf {
                    "  "
                } else if row.expanded {
                    "▾ "
                } else {
                    "▸ "
                };
                let style = data.lock().unwrap().theme.get(if row.section {
                    "ui.text.directory"
                } else {
                    "ui.text"
                });
                Span::styled(
                    format!("{}{marker}{}", "  ".repeat(row.depth), row.label),
                    style,
                )
                .into()
            }),
            ui::PickerColumn::new("location", |row: &Row, _: &Data| {
                if row.section {
                    return "".into();
                }
                let uri = &row.item.location.uri;
                let path = uri.as_path().map_or_else(
                    || uri.to_string(),
                    |path| path::get_relative_path(path).to_string_lossy().into_owned(),
                );
                format!("{}:{}", path, row.item.location.range.start.line + 1).into()
            })
            .without_filtering(),
        ],
        0,
        rows,
        data,
        move |cx, row, data, action| {
            if !accept_session.item_is_current(cx.editor, &row.item) {
                cx.editor
                    .set_error(|| "Hierarchy is no longer current; reopen it");
                return PickerCallbackResult::KeepOpen;
            }
            if row.section {
                return toggle(data, row, PickerItemAction::ToggleExpanded);
            }
            jump_to_location(cx.editor, &row.item.location, action);
            PickerCallbackResult::Close
        },
    )
    .with_item_data_actions(
        |maps| &maps.hierarchy_picker,
        move |cx, row, data, action| {
            let Some(row) = row else {
                return PickerCallbackResult::KeepOpen;
            };
            if !toggle_session.item_is_current(cx.editor, &row.item) {
                cx.editor
                    .set_error(|| "Hierarchy is no longer current; reopen it");
                return PickerCallbackResult::KeepOpen;
            }
            toggle(data, row, action)
        },
    )
    .with_item_key(|row| row.id)
    .with_editor_updates(updates)
    .with_query_options(|_, query, data, selected| {
        let mut tree = data.lock().unwrap();
        tree.query = query.to_owned();
        let rows = tree.rows();
        let cursor = selected
            .and_then(|selected| rows.iter().position(|row| row.id == selected.id))
            .unwrap_or_else(|| rows.iter().position(|row| row.matches).unwrap_or(0));
        Ok((rows, data.clone(), cursor))
    })
    .with_title(hierarchy.title())
    .with_preview(|_, row| location_to_file_location(&row.item.location))
    .with_quicklist(|_, row| {
        (!row.section)
            .then(|| location_to_quicklist_entry(&row.item.location))
            .flatten()
    })
    .truncate_start(false)
}

fn toggle(data: &Data, row: &Row, action: PickerItemAction) -> PickerCallbackResult<Row, Data> {
    let mut tree = data.lock().unwrap();
    tree.toggle(row.id, action);
    PickerCallbackResult::Refresh {
        options: tree.rows(),
        editor_data: data.clone(),
    }
}

fn updates(
    session: HierarchySession,
    data: Data,
) -> impl FnMut(UpdateContext<'_, Row>) -> Option<Vec<Row>> {
    let mut pending: Option<(
        usize,
        oneshot::Receiver<lsp_client::Result<Vec<HierarchyItem>>>,
    )> = None;
    move |cx| {
        let mut tree = data.lock().unwrap();
        if tree.stale {
            return None;
        }
        if tree
            .roots
            .iter()
            .any(|&id| !session.item_is_current(cx.editor, &tree.nodes[id].item))
        {
            cx.task.cancel();
            pending = None;
            tree.stale = true;
            *cx.status = Some("Hierarchy changed; reopen it to refresh".into());
            return Some(Vec::new());
        }
        let mut changed = false;
        let rows = tree.rows();
        if let Some((id, _)) = &pending
            && (cx.resumed || !rows.iter().any(|row| row.id == *id && row.expanded))
        {
            cx.task.cancel();
            pending = None;
            tree.loading = None;
            changed = true;
        }
        if let Some((id, receiver)) = &mut pending {
            let id = *id;
            match receiver.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(children) => tree.set_children(id, children),
                        Err(error) => {
                            tree.nodes[id].failed = true;
                            cx.editor
                                .set_error(|| format!("Hierarchy request failed: {error}"));
                        }
                    }
                    pending = None;
                    tree.loading = None;
                    changed = true;
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    pending = None;
                    tree.loading = None;
                    tree.nodes[id].failed = true;
                    changed = true;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if pending.is_none() {
            let next = tree.rows().into_iter().find(|row| {
                let node = &tree.nodes[row.id];
                row.expanded && !row.leaf && !node.failed && node.children.is_none()
            });
            if let Some(row) = next {
                let node = &tree.nodes[row.id];
                match session.children(cx.editor, &node.item, node.direction.unwrap()) {
                    Ok(request) => {
                        let cancel = cx.task.restart();
                        let redraw = cx.editor.redraw().on_drop();
                        let (sender, receiver) = oneshot::channel();
                        pending = Some((row.id, receiver));
                        tree.loading = Some(row.id);
                        tokio::spawn(async move {
                            let _redraw = redraw;
                            if let Some(result) = event::cancelable_future(request, cancel).await {
                                let _ = sender.send(result);
                            }
                        });
                    }
                    Err(error) => {
                        tree.nodes[row.id].failed = true;
                        cx.editor.set_error(|| error.to_string());
                    }
                }
                changed = true;
            }
        }
        *cx.status = Some(
            if pending.is_some() {
                "Loading hierarchy…"
            } else {
                "Expand a branch to load related symbols"
            }
            .into(),
        );
        changed.then(|| tree.rows())
    }
}
