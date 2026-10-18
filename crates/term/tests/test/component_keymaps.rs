use std::{cell::RefCell, rc::Rc};

use term::{
    application::Application,
    compositor::{Component, Context, EventResult},
    config::{Config, ConfigLoadError},
    job::Jobs,
    ui::{
        lsp::signature_help::{Signature, SignatureHelp},
        menu::Item,
        Menu, Picker, PickerColumn, Popup, Prompt, PromptEvent, Text,
    },
};
use tui::{buffer::Buffer, widgets::Row};
use ui_core::input::Event;
use view::graphics::Rect;

use super::helpers::{test_config, AppBuilder};

struct Fixture {
    app: Application,
    config: Config,
    jobs: Jobs,
    updates: tokio::sync::mpsc::UnboundedSender<term::config::ConfigEvent>,
}

impl Fixture {
    fn load_config(keys: &str) -> Config {
        Config::load(Ok(&keys.to_owned()), Err(ConfigLoadError::default())).unwrap()
    }

    fn new(keys: &str) -> anyhow::Result<Self> {
        let mut config = test_config();
        config
            .editor
            .indent
            .smart_tab
            .as_mut()
            .unwrap()
            .supersede_menu = true;
        config.component_keys = Self::load_config(keys).component_keys;
        Ok(Self {
            app: AppBuilder::new().with_config(config.clone()).build()?,
            config,
            jobs: Jobs::new(),
            updates: tokio::sync::mpsc::unbounded_channel().0,
        })
    }

    fn context(&mut self) -> Context<'_> {
        Context {
            config: term::config::Context {
                current: &self.config,
                updates: &self.updates,
            },
            editor: &mut self.app.editor,
            jobs: &mut self.jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: false,
        }
    }

    fn key(&mut self, component: &mut impl Component, key: &str) -> EventResult {
        component.handle_event(&Event::Key(key.parse().unwrap()), &mut self.context())
    }
}

fn consumed(result: EventResult) {
    assert!(matches!(result, EventResult::Consumed(None)));
}

fn closes(result: EventResult) {
    assert!(matches!(result, EventResult::Consumed(Some(_))));
}

#[derive(Clone, PartialEq)]
struct OptionItem(&'static str);

impl Item for OptionItem {
    type Data = ();
    fn format(&self, _: &()) -> Row<'_> {
        Row::new([self.0])
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_actions_preserve_unicode_typing_paste_and_callback_order() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(
        r#"
        [keys.prompt]
        C-j = ["move_start", "delete_forward"]
        C-space = ["accept", "delete_backward"]
        C-n = "no_op"
        x = "unbound"
        g = { g = "move_end" }
    "#,
    )?;
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let mut prompt = Prompt::new(
        "".into(),
        None,
        |_, _| vec![],
        move |_, line, event| {
            recorded.borrow_mut().push((line.to_owned(), event));
        },
    );
    consumed(fixture.key(&mut prompt, "é"));
    consumed(fixture.key(&mut prompt, "界"));
    consumed(fixture.key(&mut prompt, "x"));
    consumed(fixture.key(&mut prompt, "C-n"));
    consumed(fixture.key(&mut prompt, "C-z")); // Unmapped control keys do not insert text.
    assert_eq!(prompt.line(), "é界x");
    consumed(fixture.key(&mut prompt, "C-j"));
    assert_eq!(prompt.line(), "界x");
    consumed(fixture.key(&mut prompt, "g"));
    assert!(prompt.has_pending_keys());
    consumed(fixture.key(&mut prompt, "esc"));
    assert!(!prompt.has_pending_keys());
    consumed(fixture.key(&mut prompt, "g"));
    consumed(prompt.handle_event(&Event::Paste("🙂".into()), &mut fixture.context()));
    assert!(!prompt.has_pending_keys());
    assert_eq!(prompt.line(), "🙂界x");
    closes(fixture.key(&mut prompt, "C-space"));
    assert_eq!(prompt.line(), "🙂界x"); // The closing action ends the sequence.
    assert!(
        matches!(events.borrow().last(), Some((line, PromptEvent::Validate)) if line == "🙂界x")
    );
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn menu_actions_sequences_empty_options_and_smart_tab() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(
        r#"
        [keys.menu]
        C-j = ["move_down", "move_down"]
        C-n = "unbound"
        C-d = "no_op"
        tab = "accept"
    "#,
    )?;
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let mut menu = Menu::new(
        vec![OptionItem("one"), OptionItem("two")],
        (),
        move |_, item, event| {
            recorded.borrow_mut().push((item.map(|item| item.0), event));
        },
    );
    consumed(fixture.key(&mut menu, "C-j"));
    assert_eq!(menu.selection().unwrap().0, "two");
    assert_eq!(events.borrow().len(), 2);
    assert!(matches!(
        fixture.key(&mut menu, "C-n"),
        EventResult::Ignored(None)
    ));
    consumed(fixture.key(&mut menu, "C-d"));
    assert_eq!(events.borrow().len(), 2);
    // Smart tab normally supersedes Tab, but a remapped acceptance still runs.
    closes(fixture.key(&mut menu, "tab"));
    assert!(matches!(
        events.borrow().last(),
        Some((Some("two"), PromptEvent::Validate))
    ));
    let mut empty = Menu::new(Vec::<OptionItem>::new(), (), |_, _, _| {});
    consumed(fixture.key(&mut empty, "C-j"));
    consumed(fixture.key(&mut empty, "pageup"));
    consumed(fixture.key(&mut empty, "pagedown"));
    assert!(empty.selection().is_none());
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn popup_and_nested_menu_prefixes_preserve_priority_and_escape_behavior() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new(
        r#"
        [keys.menu]
        C-j = "move_down"
        g = { d = "move_down" }
        [keys.popup]
        C-j = "cancel"
        z = { tab = "cancel" }
        C-space = "cancel"
    "#,
    )?;
    let menu = Menu::new(vec![OptionItem("one")], (), |_, _, _| {});
    let mut popup = Popup::new("test", menu).ignore_escape_key(true);
    consumed(fixture.key(&mut popup, "C-j")); // Child handles it before container cancel.
    assert!(matches!(
        fixture.key(&mut popup, "esc"),
        EventResult::Ignored(None)
    ));
    consumed(fixture.key(&mut popup, "g"));
    assert!(popup.has_pending_keys());
    consumed(fixture.key(&mut popup, "esc")); // Cancels child prefix, preserving the popup.
    assert!(!popup.has_pending_keys());
    consumed(fixture.key(&mut popup, "z"));
    closes(fixture.key(&mut popup, "tab")); // Pending parent owns even a menu key.
    consumed(fixture.key(&mut popup, "g"));
    assert!(matches!(
        popup.handle_event(&Event::Paste("text".into()), &mut fixture.context()),
        EventResult::Ignored(None)
    ));
    assert!(!popup.has_pending_keys());
    consumed(fixture.key(&mut popup, "z"));
    popup.handle_event(&Event::Paste("text".into()), &mut fixture.context());
    assert!(!popup.has_pending_keys());
    closes(fixture.key(&mut popup, "C-space"));
    let mut popup = Popup::new("test", Text::new("text".into()));
    consumed(fixture.key(&mut popup, "C-d")); // Default scroll remains available.
    fixture.config = Fixture::load_config("[keys.popup]\nC-d = 'unbound'\nesc = 'no_op'");
    assert!(matches!(
        fixture.key(&mut popup, "C-d"),
        EventResult::Ignored(None)
    ));
    let mut popup = popup.ignore_escape_key(true);
    consumed(fixture.key(&mut popup, "esc")); // An explicit no_op overrides Escape passthrough.
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_routes_prompt_prefixes_before_navigation_and_split_acceptance() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new(
        r#"
        [keys.prompt]
        C-x = { tab = "move_start" }
        [keys.picker]
        C-j = "move_down"
        C-space = "accept_vertical_split"
    "#,
    )?;
    let picked = Rc::new(RefCell::new(Vec::new()));
    let recorded = picked.clone();
    let mut picker = Picker::new(
        fixture.app.editor.redraw().clone(),
        [PickerColumn::new("name", |item: &OptionItem, _| {
            item.0.into()
        })],
        0,
        [OptionItem("one"), OptionItem("two")],
        (),
        move |_, item, action| recorded.borrow_mut().push((item.0, action)),
    );
    // Rendering advances the asynchronous matcher until the options are visible.
    for _ in 0..100 {
        let area = Rect::new(0, 0, 80, 24);
        picker.render(area, &mut Buffer::empty(area), &mut fixture.context());
        if picker.selection().is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(picker.selection().unwrap().0, "one");
    // The diff toggle keeps the prompt's delete-word behavior in ordinary pickers.
    for (keys, expected) in [(vec!["t"], "two"), (vec!["C-a", "A-d"], "one")] {
        for key in keys {
            consumed(fixture.key(&mut picker, key));
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let area = Rect::new(0, 0, 80, 24);
                picker.render(area, &mut Buffer::empty(area), &mut fixture.context());
                if picker.selection().is_some_and(|item| item.0 == expected) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await?;
        assert_eq!(picker.selection().unwrap().0, expected);
    }
    consumed(fixture.key(&mut picker, "C-x"));
    consumed(fixture.key(&mut picker, "tab"));
    assert_eq!(picker.selection().unwrap().0, "one");
    consumed(fixture.key(&mut picker, "C-j"));
    assert_eq!(picker.selection().unwrap().0, "two");
    closes(fixture.key(&mut picker, "C-space"));
    assert!(matches!(
        picked.borrow().as_slice(),
        [("two", view::editor::Action::VerticalSplit)]
    ));
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn signature_actions_use_current_config_without_lsp_state_changes() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("[keys.signature-help]\nC-j = 'next'\nC-k = 'previous'")?;
    let mut signature = SignatureHelp::new(
        "text".into(),
        fixture.app.editor.syn_loader.clone(),
        0,
        Some(0),
        vec![
            Signature {
                signature: "one()".into(),
                signature_doc: None,
                active_param_range: None,
            },
            Signature {
                signature: "two()".into(),
                signature_doc: None,
                active_param_range: None,
            },
        ],
    );
    consumed(fixture.key(&mut signature, "C-j"));
    assert_eq!(signature.active_signature(), 1);
    assert_eq!(signature.lsp_signature(), Some(0));
    fixture.config = Fixture::load_config("[keys.signature-help]\nC-j = 'previous'");
    consumed(fixture.key(&mut signature, "C-j"));
    assert_eq!(signature.active_signature(), 0);
    // A completed prefix still owns its suffix when there is no page to switch to.
    let mut single = SignatureHelp::new(
        "text".into(),
        fixture.app.editor.syn_loader.clone(),
        0,
        Some(0),
        vec![Signature {
            signature: "one()".into(),
            signature_doc: None,
            active_param_range: None,
        }],
    );
    fixture.config = Fixture::load_config("[keys.signature-help]\ng = { j = 'next' }");
    consumed(fixture.key(&mut single, "g"));
    consumed(fixture.key(&mut single, "j"));
    assert!(!single.has_pending_keys());
    assert_eq!(single.active_signature(), 0);
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_register_selection_reads_literal_keys_and_cancels_on_escape() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new("[keys.prompt]\na = 'move_start'")?;
    fixture
        .app
        .editor
        .registers
        .write('a', vec!["saved text".into()])?;
    let mut prompt = Prompt::new("".into(), None, |_, _| vec![], |_, _, _| {});
    consumed(fixture.key(&mut prompt, "C-r"));
    consumed(fixture.key(&mut prompt, "a"));
    assert_eq!(prompt.line(), "saved text");
    assert!(!prompt.has_pending_keys());
    consumed(fixture.key(&mut prompt, "C-r"));
    consumed(fixture.key(&mut prompt, "esc"));
    assert!(!prompt.has_pending_keys());
    consumed(fixture.key(&mut prompt, "x"));
    assert_eq!(prompt.line(), "saved textx");
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_actions_change_the_rendered_page() -> anyhow::Result<()> {
    use lsp_client::lsp::{Hover as LspHover, HoverContents, MarkedString};
    use term::ui::lsp::hover::Hover;

    let mut fixture = Fixture::new("[keys.hover]\nC-j = 'next'\nC-k = 'previous'")?;
    let mut hover = Hover::new(
        ["first page", "second page"]
            .into_iter()
            .map(|text| {
                (
                    "server".into(),
                    LspHover {
                        contents: HoverContents::Scalar(MarkedString::String(text.into())),
                        range: None,
                    },
                )
            })
            .collect(),
        fixture.app.editor.syn_loader.clone(),
    );
    let render = |hover: &mut Hover, fixture: &mut Fixture| {
        let area = Rect::new(0, 0, 80, 24);
        let mut buffer = Buffer::empty(area);
        hover.render(area, &mut buffer, &mut fixture.context());
        buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    assert!(render(&mut hover, &mut fixture).contains("first page"));
    consumed(fixture.key(&mut hover, "C-j"));
    assert!(render(&mut hover, &mut fixture).contains("second page"));
    consumed(fixture.key(&mut hover, "C-k"));
    assert!(render(&mut hover, &mut fixture).contains("first page"));
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_popup_can_hide_borders_without_changing_layout_or_scrolling() -> anyhow::Result<()> {
    use editor_config::BorderStyle;
    use lsp_client::lsp::{Hover as LspHover, HoverContents, MarkedString};
    use term::ui::lsp::hover::Hover;
    use view::editor::ConfigEvent;

    let mut fixture = Fixture::new("")?;
    let viewport = Rect::new(0, 0, 80, 40);
    let contents = format!("first line\n\n{}last line", "middle line\n\n".repeat(40));
    for count in [1, 2] {
        let hover = Hover::new(
            (0..count)
                .map(|_| {
                    (
                        "server".into(),
                        LspHover {
                            contents: HoverContents::Scalar(MarkedString::String(contents.clone())),
                            range: None,
                        },
                    )
                })
                .collect(),
            fixture.app.editor.syn_loader.clone(),
        );
        let mut popup = Popup::new(Hover::ID, hover);
        let expected_area = popup.area(viewport, &fixture.app.editor);
        let mut buffer = Buffer::empty(viewport);
        for (style, corner) in [
            (BorderStyle::Rounded, "╭"),
            (BorderStyle::None, " "),
            (BorderStyle::Double, "╔"),
            (BorderStyle::None, " "),
        ] {
            let mut config = (*fixture.app.editor.config()).clone();
            config.border_style = style;
            fixture
                .app
                .handle_config_events(ConfigEvent::Update(Box::new(config)));

            popup.render(viewport, &mut buffer, &mut fixture.context());
            assert_eq!(popup.area(viewport, &fixture.app.editor), expected_area);
            assert_eq!(buffer[(expected_area.x, expected_area.y)].symbol(), corner);
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("first line"));
            if count > 1 {
                assert!(text.contains("server"));
            }
            if style == BorderStyle::None {
                assert!(!text.contains(['╭', '╮', '╰', '╯', '─', '│', '═', '║']));
            }
        }

        for _ in 0..20 {
            popup.scroll_half_page_down();
        }
        let mut buffer = Buffer::empty(viewport);
        popup.render(viewport, &mut buffer, &mut fixture.context());
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("last line"));
        assert!(!text.contains("first line"));
    }
    assert!(fixture.app.close().await.is_empty());
    Ok(())
}
