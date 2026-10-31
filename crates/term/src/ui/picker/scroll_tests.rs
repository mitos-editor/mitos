use super::{
    test::{application, until},
    *,
};
use crate::{application::Application, config::Config, job::Jobs};
use std::path::PathBuf;
use ui_core::input::{KeyModifiers, MouseEvent, MouseEventKind};

struct Location {
    target: QuicklistTarget,
    range: Option<(usize, usize)>,
}

struct Fixture {
    app: Application,
    config: Config,
    picker: Picker<Location, ()>,
}

impl Fixture {
    async fn new(locations: Vec<Location>) -> anyhow::Result<Self> {
        Self::with_app(application(false)?, locations).await
    }

    async fn with_app(mut app: Application, locations: Vec<Location>) -> anyhow::Result<Self> {
        let mut picker = Picker::new(
            app.editor.redraw().clone(),
            [Column::new("name", |_: &Location, _| "result".into())],
            0,
            locations,
            (),
            |_, _, _| {},
        )
        .with_preview(|_, item| {
            Some((
                match &item.target {
                    QuicklistTarget::Path(path) => PathOrId::Path(path),
                    QuicklistTarget::Document(id) => PathOrId::Id(*id),
                },
                item.range,
            ))
        });
        until(&mut picker, &mut app, |picker| picker.selection().is_some()).await?;
        Ok(Self {
            app,
            config: Config::default(),
            picker,
        })
    }

    fn event(&mut self, event: Event) {
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut jobs = Jobs::new();
        self.picker.handle_event(
            &event,
            &mut Context {
                config: crate::config::Context {
                    current: &self.config,
                    updates: &updates,
                },
                editor: &mut self.app.editor,
                jobs: &mut jobs,
                scroll: None,
                image_picker: None,
                is_cursor_owner: true,
            },
        );
    }

    fn key(&mut self, key: &str) {
        self.event(Event::Key(key.parse().unwrap()));
    }

    fn draw(&mut self, width: u16, height: u16) -> Surface {
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut jobs = Jobs::new();
        let area = Rect::new(0, 0, width, height);
        let mut surface = Surface::empty(area);
        self.picker.render(
            area,
            &mut surface,
            &mut Context {
                config: crate::config::Context {
                    current: &self.config,
                    updates: &updates,
                },
                editor: &mut self.app.editor,
                jobs: &mut jobs,
                scroll: None,
                image_picker: None,
                is_cursor_owner: true,
            },
        );
        surface
    }

    fn preview_text(&mut self) -> String {
        let surface = self.draw(100, 20);
        (1..19)
            .map(|y| {
                (52..98)
                    .map(|x| surface[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn top(&mut self) -> String {
        let surface = self.draw(100, 20);
        (52..97)
            .map(|x| surface[(x, 1)].symbol())
            .collect::<String>()
            .trim()
            .to_owned()
    }
}

fn location(path: PathBuf, range: Option<(usize, usize)>) -> Location {
    Location {
        target: QuicklistTarget::Path(path),
        range,
    }
}

fn lines(count: usize) -> String {
    (0..count).map(|i| format!("line-{i:03}\n")).collect()
}

#[tokio::test]
async fn file_preview_keys_scroll_half_pages_and_clamp_without_changing_the_result(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, lines(100))?;
    let mut fixture = Fixture::new(vec![location(path, None)]).await?;
    fixture.app.editor.theme = theme::Theme::from(toml::Value::Table(toml::toml! {
        "ui.menu.scroll" = { fg = "red", bg = "blue" }
    }));
    assert_eq!(fixture.top(), "line-000");
    let start = fixture.draw(100, 20);
    assert_eq!(start[(97, 1)].fg, view::graphics::Color::Red);
    assert_eq!(start[(97, 18)].fg, view::graphics::Color::Blue);
    let corner = start[(99, 0)].symbol().to_owned();
    for key in ["A-j", "A-down"] {
        fixture.key(key);
        assert_eq!(fixture.top(), "line-009");
        fixture.key("A-k");
        assert_eq!(fixture.top(), "line-000");
    }
    for _ in 0..100 {
        fixture.key("A-j");
    }
    assert_eq!(fixture.top(), "line-083");
    let end = fixture.draw(100, 20);
    assert_eq!(end[(97, 1)].fg, view::graphics::Color::Blue);
    assert_eq!(end[(97, 18)].fg, view::graphics::Color::Red);
    assert_eq!(end[(99, 0)].symbol(), corner);
    assert!(fixture.preview_text().contains("line-099"));
    fixture.key("A-up");
    assert_eq!(fixture.top(), "line-074");
    assert_eq!(fixture.picker.results.cursor, 0);
    assert_eq!(fixture.picker.prompt.line(), "");
    Ok(())
}

#[tokio::test]
async fn selecting_another_match_in_the_same_file_recenters_the_preview() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, lines(100))?;
    let mut fixture = Fixture::new(vec![
        location(path.clone(), Some((40, 40))),
        location(path, Some((70, 70))),
    ])
    .await?;
    assert_eq!(fixture.top(), "line-031");
    fixture.key("A-j");
    assert_eq!(fixture.top(), "line-040");
    fixture.key("down");
    assert_eq!(fixture.top(), "line-061");
    fixture.key("up");
    assert_eq!(fixture.top(), "line-031");
    Ok(())
}

#[tokio::test]
async fn directory_previews_scroll_and_fitting_or_placeholder_previews_ignore_scroll(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for i in 0..50 {
        std::fs::write(dir.path().join(format!("entry-{i:03}")), "")?;
    }
    let mut fixture = Fixture::new(vec![location(dir.path().to_owned(), None)]).await?;
    assert!(fixture.preview_text().contains("entry-000"));
    fixture.key("A-j");
    assert!(!fixture.preview_text().contains("entry-000"));
    assert!(fixture.preview_text().contains("entry-020"));
    for _ in 0..100 {
        fixture.key("A-j");
    }
    assert!(fixture.preview_text().contains("entry-049"));
    fixture.key("A-k");
    assert!(!fixture.preview_text().contains("entry-049"));

    let small = dir.path().join("small");
    std::fs::write(&small, "fits\n")?;
    let mut fixture = Fixture::new(vec![location(small, None)]).await?;
    assert_eq!(fixture.top(), "fits");
    fixture.key("A-j");
    assert_eq!(fixture.top(), "fits");
    assert_eq!(fixture.picker.preview_scroll.scroll.offset, 0);
    assert!(!fixture.preview_text().contains('▐'));
    let binary = dir.path().join("binary");
    std::fs::write(&binary, [0, 0, 0])?;
    for (path, placeholder) in [
        (binary, "<Binary file>"),
        (dir.path().join("missing"), "<File not found>"),
    ] {
        let mut fixture = Fixture::new(vec![location(path, None)]).await?;
        let before = fixture.preview_text();
        assert!(before.contains(placeholder));
        fixture.key("A-j");
        assert_eq!(fixture.preview_text(), before);
    }
    Ok(())
}

#[tokio::test]
async fn mouse_scrolling_is_limited_to_the_visible_preview_and_keys_can_be_remapped(
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, lines(100))?;
    let mut fixture = Fixture::new(vec![location(path, None)]).await?;
    fixture.draw(100, 20);
    for (column, expected) in [(20, "line-000"), (60, "line-009")] {
        fixture.event(Event::Mouse(MouseEvent {
            column,
            row: 5,
            kind: MouseEventKind::ScrollDown,
            modifiers: KeyModifiers::empty(),
        }));
        assert_eq!(fixture.top(), expected);
    }
    fixture.key("C-t");
    fixture.draw(100, 20);
    fixture.key("A-j");
    fixture.key("C-t");
    assert_eq!(fixture.top(), "line-009");
    fixture.draw(60, 20);
    fixture.key("A-j");
    assert_eq!(fixture.top(), "line-009");
    fixture.config = Config::load(
        Ok(&"[keys.picker]\nA-j = 'no_op'\nA-f = 'preview_scroll_down'\n".to_owned()),
        Err(Default::default()),
    )
    .unwrap();
    fixture.key("A-f");
    assert_eq!(fixture.top(), "line-018");
    fixture.key("A-j");
    assert_eq!(fixture.top(), "line-018");
    Ok(())
}

#[tokio::test]
async fn wrapped_unicode_preview_scrolls_visual_rows_and_clamps_after_resize() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(
        &path,
        (0..500)
            .map(|i| format!("中{i:03} 😀 "))
            .collect::<String>(),
    )?;
    let mut fixture = Fixture::new(vec![location(path.clone(), None)]).await?;
    let mut settings = (*fixture.app.editor.config()).clone();
    settings.soft_wrap.enable = Some(true);
    fixture.app.editor.config = Arc::new(arc_swap::ArcSwap::from_pointee(settings));
    let first = fixture.top();
    fixture.key("A-j");
    let second = fixture.top();
    assert_ne!(first, second);
    assert_eq!(fixture.picker.preview_scroll.scroll.offset, 9);
    fixture.key("A-k");
    assert_eq!(fixture.top(), first);
    for _ in 0..1000 {
        fixture.key("A-j");
    }
    let bottom = fixture.top();
    assert_ne!(bottom, first);
    fixture.key("A-k");
    assert_ne!(fixture.top(), bottom);
    fixture.draw(100, 1000);
    assert_eq!(fixture.picker.preview_scroll.scroll.offset, 0);
    assert_eq!(fixture.top(), first);
    fixture.app.editor.open(&path, Action::Replace)?;
    let (view, doc) = view::current!(fixture.app.editor);
    let change = editor_core::Transaction::change(
        doc.text(),
        [(0, doc.text().len_chars(), Some("short".into()))].into_iter(),
    );
    view::editing::apply(doc, &change, view.id)?;
    assert_eq!(fixture.top(), "short");
    fixture.key("A-j");
    assert_eq!(fixture.top(), "short");
    assert!(!fixture.preview_text().contains('▐'));
    Ok(())
}

#[tokio::test]
async fn open_buffer_previews_scroll_without_changing_the_editor_view() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, lines(100))?;
    let mut app = application(false)?;
    let id = app.editor.open(&path, Action::Replace)?;
    let (view, doc) = view::current!(app.editor);
    let change = editor_core::Transaction::change(
        doc.text(),
        [(0, 0, Some("unsaved\n".into()))].into_iter(),
    );
    view::editing::apply(doc, &change, view.id)?;
    let view_id = view.id;
    let offset = doc.view_offset(view_id);
    let selection = doc.selection(view_id).clone();
    let mut fixture = Fixture::with_app(
        app,
        vec![Location {
            target: QuicklistTarget::Document(id),
            range: None,
        }],
    )
    .await?;
    assert_eq!(fixture.top(), "unsaved");
    fixture.key("A-j");
    assert_eq!(fixture.top(), "line-008");
    let doc = fixture.app.editor.document(id).unwrap();
    assert_eq!(doc.view_offset(view_id), offset);
    assert_eq!(doc.selection(view_id), &selection);
    assert_eq!(std::fs::read_to_string(path)?, lines(100));
    Ok(())
}
