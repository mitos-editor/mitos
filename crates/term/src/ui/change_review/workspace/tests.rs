use super::*;
use crate::{
    application::Application, compositor::Compositor, config::Config, job::Jobs,
    ui::picker::test::application,
};
use editor_core::{Rope, Transaction};
use view::features::change_review::DiffEngine;

struct Fixture {
    app: Application,
    config: Config,
    jobs: Jobs,
    compositor: Compositor,
    updates: tokio::sync::mpsc::UnboundedSender<crate::config::ConfigEvent>,
    dir: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.txt");
        let mut app = application(false)?;
        std::fs::write(&path, "head\nkeep\n")?;
        let id = app.editor.open(&path, Action::Replace)?;
        let path = app.editor.document(id).unwrap().path().unwrap().to_owned();
        let fixture = Self {
            app,
            config: Config::default(),
            jobs: Jobs::new(),
            compositor: Compositor::new(Rect::new(0, 0, 100, 20)),
            updates: tokio::sync::mpsc::unbounded_channel().0,
            dir,
            path,
        };
        fixture.git(&["init"]);
        fixture.git(&["config", "core.autocrlf", "false"]);
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "initial"]);
        Ok(fixture)
    }

    fn git(&self, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.autocrlf=false",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
            ])
            .args(args)
            .env_remove("GIT_DIR")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn replace(&mut self, text: &str) {
        let editor = &mut self.app.editor;
        let (view, doc) = view::current!(editor);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        view::editing::apply(doc, &transaction, view.id);
    }

    fn open(&mut self, comparison: ReviewComparison) {
        let status = vcs::FileStatus {
            staged: None,
            unstaged: Some(vcs::FileChange::Modified {
                path: self.path.clone(),
            }),
        };
        let diff = DiffView::open(
            &self.app.editor,
            status,
            self.dir.path().to_owned(),
            comparison,
        );
        self.compositor.push(Box::new(diff));
    }

    fn diff(&mut self) -> &mut DiffView {
        self.compositor.find::<DiffView>().unwrap()
    }

    fn draw(&mut self, area: Rect) -> Buffer {
        let mut surface = Buffer::empty(area);
        self.compositor.render(
            area,
            &mut surface,
            &mut Context {
                config: crate::config::Context {
                    current: &self.config,
                    updates: &self.updates,
                },
                editor: &mut self.app.editor,
                scroll: None,
                jobs: &mut self.jobs,
                image_picker: None,
                is_cursor_owner: true,
            },
        );
        surface
    }

    fn event(&mut self, event: Event) {
        assert!(self.compositor.handle_event(
            &event,
            &mut Context {
                config: crate::config::Context {
                    current: &self.config,
                    updates: &self.updates
                },
                editor: &mut self.app.editor,
                scroll: None,
                jobs: &mut self.jobs,
                image_picker: None,
                is_cursor_owner: true
            }
        ));
    }

    fn preview_open(&mut self) {
        use crate::ui::picker::{PickerPreview, PreviewAction};
        let status = vcs::FileStatus {
            staged: None,
            unstaged: Some(vcs::FileChange::Modified {
                path: self.path.clone(),
            }),
        };
        let mut preview = super::super::ChangeReviewPreview::new(
            self.dir.path().to_owned(),
            |status: &vcs::FileStatus| status,
        );
        let mut cx = Context {
            config: crate::config::Context {
                current: &self.config,
                updates: &self.updates,
            },
            editor: &mut self.app.editor,
            scroll: None,
            jobs: &mut self.jobs,
            image_picker: None,
            is_cursor_owner: true,
        };
        let EventResult::Consumed(Some(callback)) =
            preview.action(Some(&status), PreviewAction::Open, &mut cx)
        else {
            panic!("preview must open a diff view");
        };
        callback(&mut self.compositor, &mut cx);
    }
    fn key(&mut self, key: &str) {
        self.event(Event::Key(key.parse().unwrap()));
    }

    async fn ready(&mut self) -> anyhow::Result<Buffer> {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let surface = self.draw(Rect::new(0, 0, 100, 20));
                if self.diff().view.pending.is_none() && self.diff().view.mode_pending.is_none() {
                    break surface;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(Into::into)
    }
}

#[tokio::test]
async fn diff_workspace_is_read_only_navigable_and_safe_to_resize() -> anyhow::Result<()> {
    let mut f = Fixture::new()?;
    f.replace("unsaved\nkeep\n");
    let index = std::fs::read(f.dir.path().join(".git/index"))?;
    f.open(ReviewComparison::Head);
    let surface = f.ready().await?;
    let text: String = surface.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("HEAD → buffer"));
    assert!(text.contains("+1 −1"));
    assert!(text.contains("1   − head"));
    assert!(text.contains("  1 + unsaved"));
    let added = f
        .diff()
        .view
        .review
        .lines
        .iter()
        .position(|line| line.kind == ReviewLineKind::Added)
        .unwrap();
    let expected_bg = super::super::line_style(&f.app.editor.theme, ReviewLineKind::Added)
        .bg
        .unwrap();
    assert_eq!(surface[(99, (added + 1) as u16)].bg, expected_bg.into());
    f.key("end");
    let diff = f.diff();
    assert_eq!(diff.selected, diff.view.review.lines.len() - 1);
    f.key("home");
    f.key("j");
    f.key("k");
    assert_eq!(f.diff().selected, 0);
    f.key("i");
    f.key("d");
    f.event(Event::Paste("must not edit".into()));
    assert_eq!(
        view::current_ref!(f.app.editor).1.text().to_string(),
        "unsaved\nkeep\n"
    );
    for width in [0, 1, 2, 8, 25, 80] {
        for height in [0, 1, 2, 3, 6] {
            f.draw(Rect::new(2, 3, width, height));
        }
    }
    f.key("esc");
    assert!(f.compositor.find::<DiffView>().is_none());
    assert_eq!(std::fs::read_to_string(&f.path)?, "head\nkeep\n");
    assert_eq!(std::fs::read(f.dir.path().join(".git/index"))?, index);
    Ok(())
}

#[tokio::test]
async fn refresh_and_engine_switches_use_current_requests_without_losing_unsaved_edits(
) -> anyhow::Result<()> {
    let mut f = Fixture::new()?;
    f.replace("first\nkeep\n");
    f.open(ReviewComparison::Head);
    // Toggle twice before the initial request has published.
    f.key("A-d");
    f.key("A-d");
    f.ready().await?;
    assert_eq!(f.diff().view.engine, Some(DiffEngine::Line));
    f.replace("second\nkeep\n");
    assert!(f
        .diff()
        .view
        .review
        .lines
        .iter()
        .any(|line| line.text == "first"));
    f.key("r");
    f.key("A-d");
    f.ready().await?;
    assert!(f
        .diff()
        .view
        .review
        .lines
        .iter()
        .any(|line| line.text == "second"));
    assert_eq!(f.diff().view.engine, Some(DiffEngine::Syntax));
    assert!(f.diff().view.review.fallback.is_some());
    f.key("A-right");
    f.ready().await?;
    assert_eq!(f.diff().comparison, ReviewComparison::Staged);
    assert_eq!(f.diff().view.review.additions, 0);
    f.key("A-left");
    f.ready().await?;
    assert_eq!(f.diff().comparison, ReviewComparison::Head);
    assert_eq!(
        view::current_ref!(f.app.editor).1.text().to_string(),
        "second\nkeep\n"
    );
    Ok(())
}

#[tokio::test]
async fn source_jumps_reject_stale_snapshots_and_preserve_jump_history() -> anyhow::Result<()> {
    let mut f = Fixture::new()?;
    f.replace("keep\n");
    f.open(ReviewComparison::Head);
    f.ready().await?;
    let removed = f
        .diff()
        .view
        .review
        .lines
        .iter()
        .position(|line| line.kind == ReviewLineKind::Removed)
        .unwrap();
    f.diff().selected = removed;
    f.replace("newer\nkeep\n");
    f.key("ret");
    assert!(f.compositor.find::<DiffView>().is_some());
    assert!(f
        .app
        .editor
        .status_msg
        .as_ref()
        .unwrap()
        .0
        .contains("differs"));
    f.replace("keep\n");
    f.key("ret");
    assert!(f.compositor.find::<DiffView>().is_none());
    let (view, doc) = view::current_ref!(f.app.editor);
    assert_eq!(doc.cursor_line(view.id), 0);
    assert!(view.jumps.iter().next().is_some());
    Ok(())
}

#[tokio::test]
async fn deleted_files_are_reviewable_without_recreating_them() -> anyhow::Result<()> {
    let mut f = Fixture::new()?;
    // Review a path that is not an open editor document.
    let deleted = f.dir.path().join("deleted.txt");
    std::fs::write(&deleted, "removed\n")?;
    f.git(&["add", "."]);
    f.git(&["commit", "-m", "second"]);
    std::fs::remove_file(&deleted)?;
    f.path = deleted;
    let before = f.app.editor.documents().count();
    f.open(ReviewComparison::Unstaged);
    f.ready().await?;
    assert_eq!(f.diff().view.review.deletions, 1);
    f.key("j");
    f.key("ret");
    f.key("o");
    assert!(f.compositor.find::<DiffView>().is_some());
    assert!(!f.path.exists());
    assert_eq!(f.app.editor.documents().count(), before);
    Ok(())
}

#[tokio::test]
async fn picker_diff_returns_to_picker_on_close_and_to_editor_on_open() -> anyhow::Result<()> {
    let mut f = Fixture::new()?;
    std::fs::write(&f.path, "disk\nkeep\n")?;
    let picker = crate::ui::Picker::new(
        f.app.editor.redraw().clone(),
        [crate::ui::PickerColumn::new("test", |_: &(), _: &()| {
            "".into()
        })],
        0,
        [()],
        (),
        |_, _, _| {},
    );
    f.compositor
        .push(Box::new(crate::ui::overlay::overlaid(picker)));
    f.preview_open();
    f.ready().await?;
    assert_eq!(f.compositor.layer_count(), 2);
    f.key("esc");
    assert_eq!(f.compositor.layer_count(), 1);
    f.preview_open();
    f.ready().await?;
    f.key("o");
    assert_eq!(f.compositor.layer_count(), 0);
    assert_eq!(
        view::current_ref!(f.app.editor).1.path(),
        Some(f.path.as_path())
    );
    Ok(())
}

#[cfg(feature = "syntax-diff")]
#[tokio::test]
async fn language_policy_applies_syntax_emphasis_in_the_full_diff_view() -> anyhow::Result<()> {
    let mut f = Fixture::new()?;
    f.app
        .editor
        .syn_loader
        .store(Arc::new(language::Loader::new(
            toml::from_str(
                r#"
        [[language]]
        name = "review-json"
        scope = "source.json"
        file-types = ["json"]
        grammar = "json"
        diff-engine = "syntax"
    "#,
            )?,
            Default::default(),
        )?));
    let path = f.dir.path().join("sample.json");
    std::fs::write(&path, "{\"value\": 1}\n")?;
    f.git(&["add", "."]);
    f.git(&["commit", "-m", "json"]);
    let id = f.app.editor.open(&path, Action::Replace)?;
    f.path = f
        .app
        .editor
        .document(id)
        .unwrap()
        .path()
        .unwrap()
        .to_owned();
    f.replace("{\"value\": 2}\n");
    f.open(ReviewComparison::Head);
    let surface = f.ready().await?;
    assert_eq!(f.diff().view.review.provider, Some("syntax"));
    assert_eq!(f.diff().view.engine, Some(DiffEngine::Syntax));
    let row = f
        .diff()
        .view
        .review
        .lines
        .iter()
        .position(|line| line.kind == ReviewLineKind::Added)
        .unwrap();
    let line: Vec<_> = (0..100).map(|x| &surface[(x, row as u16 + 1)]).collect();
    assert!(line
        .iter()
        .any(|cell| cell.symbol() == "2" && cell.modifier.contains(tui::style::Modifier::BOLD)));
    assert!(line
        .iter()
        .any(|cell| cell.symbol() == "v" && !cell.modifier.contains(tui::style::Modifier::BOLD)));
    f.key("A-d");
    f.ready().await?;
    assert_eq!(f.diff().view.review.provider, None);
    assert_eq!(f.diff().view.engine, Some(DiffEngine::Line));
    Ok(())
}

#[test]
fn hunk_navigation_and_page_bounds_follow_the_rendered_review() {
    let before: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let after = before
        .replace("line 2\n", "new two\n")
        .replace("line 40\n", "new forty\n");
    let review = text_review(&before, &after);
    let status = vcs::FileStatus {
        staged: None,
        unstaged: Some(vcs::FileChange::Modified {
            path: "file".into(),
        }),
    };
    let mut diff = DiffView::new(
        ReviewView::new("test".into(), review),
        status,
        PathBuf::new(),
        ReviewComparison::Head,
    );
    diff.hunk(true);
    assert!(diff.selected > 0);
    diff.hunk(false);
    assert_eq!(diff.selected, 0);
    diff.height = 3;
    diff.page(true, diff.height);
    diff.clamp();
    assert_eq!((diff.selected, diff.view.offset), (3, 3));
    diff.page(false, diff.height);
    diff.clamp();
    assert_eq!((diff.selected, diff.view.offset), (0, 0));
    diff.selected = usize::MAX;
    diff.clamp();
    assert_eq!(diff.selected, diff.view.review.lines.len() - 1);
    assert_eq!(diff.view.offset + diff.height, diff.view.review.lines.len());
}

fn text_review(before: &str, after: &str) -> view::features::change_review::Review {
    view::features::change_review::Review::new(Rope::from(before), Rope::from(after), None)
}
