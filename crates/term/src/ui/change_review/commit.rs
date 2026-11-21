//! One scrollable commit message and patch, independent of the working tree.
use crate::{
    compositor::{Context, DrawContext, EventResult},
    keymap::component::{action_hint, PickerAction},
    ui::picker::{
        scroll::{self, PreviewScroll},
        PickerPreview, PreviewAction,
    },
};
use event::TaskController;
use language::{Loader, Syntax};
use std::{sync::Arc, time::Duration};
use tokio::sync::{oneshot, Semaphore};
use tui::{
    buffer::Buffer,
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};
use vcs::operations::{
    history::{CommitEntry, HistoryFile, RevisionId},
    GitRepository,
};
use view::{
    features::change_review::{HistoricalReviewRequest, Review, ReviewLineKind, ReviewSource},
    graphics::Rect,
};

type CommitDetails = (String, Vec<HistoryFile>);
type FileReviews = Vec<FileReview>;

struct FileReview {
    file: HistoryFile,
    review: Arc<Review>,
    source: Option<Arc<ReviewSource>>,
}

struct CommitMessage {
    text: String,
    syntax: Option<Syntax>,
    loader: Arc<Loader>,
}

impl CommitMessage {
    fn plain(text: String, loader: Arc<Loader>) -> Self {
        Self {
            text: text.replace("\r\n", "\n"),
            syntax: None,
            loader,
        }
    }

    fn new(text: String, loader: Arc<Loader>) -> Self {
        let mut message = Self::plain(text, loader);
        message.syntax = message
            .loader
            .language_for_name("git-commit")
            .and_then(|language| {
                Syntax::new(
                    editor_core::RopeSlice::from(message.text.as_str()),
                    language,
                    &message.loader,
                )
                .ok()
            });
        message
    }

    fn lines(&self, theme: &theme::Theme) -> Vec<Line<'static>> {
        if self.loader.scopes().as_slice() != theme.scopes() {
            self.loader.set_scopes(theme.scopes().to_vec());
        }
        let text = editor_core::RopeSlice::from(self.text.as_str());
        let highlighter = self
            .syntax
            .as_ref()
            .map(|syntax| syntax.highlighter(text, &self.loader, ..));
        let mut lines = Vec::new();
        let mut spans = Vec::new();
        for (range, style) in crate::ui::highlights::styled_spans(
            highlighter,
            None,
            theme,
            theme.get("ui.text"),
            self.text.len(),
        ) {
            for fragment in self.text[range].split_inclusive('\n') {
                let line = fragment.strip_suffix('\n');
                spans.push(Span::styled(
                    super::literal(line.unwrap_or(fragment)),
                    style,
                ));
                if line.is_some() {
                    lines.push(Line::from(std::mem::take(&mut spans)));
                }
            }
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
        lines
    }
}

pub struct CommitReviewPreview {
    repository: GitRepository,
    selected: Option<RevisionId>,
    task: TaskController,
    details: Option<oneshot::Receiver<anyhow::Result<CommitDetails>>>,
    pending: Option<oneshot::Receiver<FileReviews>>,
    message: Option<CommitMessage>,
    message_worker: Arc<Semaphore>,
    message_task: TaskController,
    highlighted_message: Option<oneshot::Receiver<CommitMessage>>,
    reviews: Option<FileReviews>,
    error: Option<String>,
    scroll: PreviewScroll,
    opened: bool,
    rendered: Option<(u64, Paragraph<'static>)>,
}

impl CommitReviewPreview {
    pub fn new(repository: GitRepository) -> Self {
        Self {
            repository,
            selected: None,
            task: TaskController::new(),
            details: None,
            pending: None,
            message: None,
            message_worker: Arc::new(Semaphore::new(1)),
            message_task: TaskController::new(),
            highlighted_message: None,
            reviews: None,
            error: None,
            scroll: PreviewScroll::default(),
            opened: false,
            rendered: None,
        }
    }

    fn highlight_message(&mut self, message: &CommitMessage, cx: &Context) {
        let text = message.text.clone();
        let loader = message.loader.clone();
        let worker = self.message_worker.clone();
        let cancel = self.message_task.restart();
        let redraw = cx.editor.redraw().clone();
        let (tx, rx) = oneshot::channel();
        self.highlighted_message = Some(rx);
        tokio::spawn(async move {
            let work = async {
                let Ok(permit) = worker.acquire_owned().await else {
                    return;
                };
                let check = cancel.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    if check.is_canceled() {
                        return None;
                    }
                    Some(CommitMessage::new(text, loader))
                })
                .await;
                if !cancel.is_canceled()
                    && let Ok(Some(message)) = result
                {
                    let _ = tx.send(message);
                    redraw.request();
                }
            };
            event::cancelable_future(work, &cancel).await;
        });
    }

    fn load_reviews(
        &mut self,
        commit: &CommitEntry,
        files: Vec<HistoryFile>,
        cx: &Context,
    ) -> anyhow::Result<()> {
        let requests = files
            .into_iter()
            .map(|file| {
                let (before, after) = file.endpoints(commit, 0)?;
                let request = HistoricalReviewRequest::revisions(
                    cx.editor,
                    self.repository.clone(),
                    before,
                    after,
                )?;
                Ok((file, request))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let cancel = self.task.restart();
        let redraw = cx.editor.redraw().clone();
        let (tx, rx) = oneshot::channel();
        self.pending = Some(rx);
        tokio::spawn(async move {
            let work = async {
                let mut reviews = Vec::new();
                let mut bytes = 0;
                let mut rows = 0;
                let mut source_bytes = 0;
                for (file, request) in requests {
                    let (review, source) = match request.run(cancel.clone()).await {
                        Ok(snapshot) => {
                            let size = snapshot.source.byte_len();
                            // Full source context can greatly exceed the displayed patch.
                            // Keep the remaining files readable without retaining their trees.
                            let source = if source_bytes + size <= vcs::MAX_REVIEW_BYTES {
                                source_bytes += size;
                                Some(snapshot.source.clone())
                            } else {
                                None
                            };
                            (snapshot.review.clone(), source)
                        }
                        Err(error) => (Arc::new(Review::notice(&error.to_string())), None),
                    };
                    bytes += review
                        .lines
                        .iter()
                        .map(|line| line.text.len())
                        .sum::<usize>();
                    rows += review.lines.len() + 4;
                    // Bound the combined patch as well as each individual file.
                    if bytes > vcs::MAX_REVIEW_BYTES || rows > 20_000 {
                        reviews.push(FileReview {
                            file,
                            review: Arc::new(Review::notice(
                                "Commit preview truncated (limit 10 MiB or 20,000 rows)",
                            )),
                            source: None,
                        });
                        break;
                    }
                    reviews.push(FileReview {
                        file,
                        review,
                        source,
                    });
                }
                if !cancel.is_canceled() {
                    let _ = tx.send(reviews);
                    redraw.request();
                }
            };
            event::cancelable_future(work, &cancel).await;
        });
        Ok(())
    }

    fn paragraph(&mut self, commit: &CommitEntry, theme: &theme::Theme) -> Paragraph<'static> {
        match self.rendered.take() {
            Some((revision, paragraph)) if revision == theme.revision() => paragraph,
            _ => crate::ui::text::paragraph(self.build_text(commit, theme)),
        }
    }

    fn build_text(&self, commit: &CommitEntry, theme: &theme::Theme) -> Text<'static> {
        let normal = theme.get("ui.text");
        let heading = theme.get("ui.text.focus");
        let mut lines = vec![
            Line::styled(format!("commit {}", commit.id.as_str()), heading),
            Line::styled(format!("Author: {}", commit.author), normal),
            Line::styled(format!("Date: {}", commit.date), normal),
            Line::default(),
        ];
        if let Some(message) = &self.message {
            lines.extend(message.lines(theme));
        } else {
            lines.extend(
                commit
                    .subject
                    .lines()
                    .map(|line| Line::styled(super::literal(line), normal)),
            );
        }
        lines.push(Line::default());
        if let Some(reviews) = &self.reviews {
            if reviews.is_empty() {
                lines.push(Line::styled("No changed files", normal));
            }
            for FileReview {
                file,
                review,
                source,
            } in reviews
            {
                let before = file.before.as_ref().map_or_else(
                    || "/dev/null".into(),
                    |path| format!("a/{}", path.display()),
                );
                let after = file.after.as_ref().map_or_else(
                    || "/dev/null".into(),
                    |path| format!("b/{}", path.display()),
                );
                lines.push(Line::styled(
                    super::literal(&format!("{} {}", file.status, file.path().display())),
                    heading,
                ));
                lines.push(Line::styled(
                    super::literal(&format!("--- {before}")),
                    heading,
                ));
                lines.push(Line::styled(
                    super::literal(&format!("+++ {after}")),
                    heading,
                ));
                for line in &review.lines {
                    let (prefix, style) = match line.kind {
                        ReviewLineKind::Added => ("+", theme.get("diff.plus")),
                        ReviewLineKind::Removed => ("-", theme.get("diff.minus")),
                        ReviewLineKind::Context => (" ", normal),
                        ReviewLineKind::Header => ("", theme.get("diff.delta")),
                        ReviewLineKind::Notice => ("", theme.get("ui.text.inactive")),
                    };
                    let syntax = source
                        .as_ref()
                        .and_then(|source| super::document::row_syntax_styles(source, line, theme));
                    let mut spans = vec![Span::styled(prefix, style)];
                    spans.extend(super::source_spans(
                        &line.text,
                        line.changes.as_deref(),
                        syntax.as_deref(),
                        0,
                        usize::MAX,
                        if syntax.is_some() || line.changes.is_some() {
                            normal
                        } else {
                            style
                        },
                        if syntax.is_some() {
                            theme::Style::default().add_modifier(theme::Modifier::BOLD)
                        } else {
                            style.add_modifier(theme::Modifier::BOLD)
                        },
                    ));
                    lines.push(Line::from(spans).style(super::line_style(theme, line.kind)));
                }
                lines.push(Line::default());
            }
        } else {
            lines.push(Line::styled(
                self.error
                    .as_deref()
                    .unwrap_or("Loading changes…")
                    .to_owned(),
                theme.get("ui.text.inactive"),
            ));
        }
        Text::from(lines)
    }
}

impl PickerPreview<Arc<CommitEntry>> for CommitReviewPreview {
    fn cancel(&mut self) {
        self.task.cancel();
        self.message_task.cancel();
        self.highlighted_message = None;
        self.details = None;
        self.pending = None;
        self.selected = None;
        self.message = None;
        self.reviews = None;
        self.error = None;
        self.scroll = PreviewScroll::default();
        self.opened = false;
        self.rendered = None;
    }

    fn close_on_open(&self) -> bool {
        self.opened
    }

    fn update(&mut self, item: Option<&Arc<CommitEntry>>, _area: Rect, cx: &mut Context) {
        let Some(commit) = item else {
            self.cancel();
            return;
        };
        if self.selected.as_ref() != Some(&commit.id) {
            self.cancel();
            self.selected = Some(commit.id.clone());
            let repository = self.repository.clone();
            let commit = commit.clone();
            let cancel = self.task.restart();
            let redraw = cx.editor.redraw().clone();
            let (tx, rx) = oneshot::channel();
            self.details = Some(rx);
            tokio::spawn(async move {
                let work = async {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    let result = tokio::try_join!(
                        repository.commit_message(&commit.id),
                        repository.commit_files(&commit, 0)
                    );
                    if !cancel.is_canceled() {
                        let _ = tx.send(result);
                        redraw.request();
                    }
                };
                event::cancelable_future(work, &cancel).await;
            });
        }
        if let Some(pending) = &mut self.details {
            match pending.try_recv() {
                Ok(result) => {
                    self.rendered = None;
                    self.details = None;
                    match result {
                        Ok((message, files)) => {
                            let message =
                                CommitMessage::plain(message, cx.editor.syn_loader.load_full());
                            self.highlight_message(&message, cx);
                            self.message = Some(message);
                            if files.is_empty() {
                                self.reviews = Some(Vec::new());
                            } else if let Err(error) = self.load_reviews(commit, files, cx) {
                                self.error = Some(error.to_string());
                            }
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.rendered = None;
                    self.details = None;
                    self.error = Some("Commit preview unavailable".into());
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if let Some(pending) = &mut self.highlighted_message {
            match pending.try_recv() {
                Ok(message) => {
                    self.message = Some(message);
                    self.highlighted_message = None;
                    self.rendered = None;
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    // Highlighting is optional; keep the plain message on failure.
                    self.highlighted_message = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if let Some(pending) = &mut self.pending {
            match pending.try_recv() {
                Ok(reviews) => {
                    self.rendered = None;
                    self.pending = None;
                    self.reviews = Some(reviews);
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.rendered = None;
                    self.pending = None;
                    self.error = Some("Commit preview unavailable".into());
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
    }

    fn render(
        &mut self,
        item: Option<&Arc<CommitEntry>>,
        area: Rect,
        surface: &mut Buffer,
        cx: &mut DrawContext,
    ) {
        let Some(commit) = item else {
            return;
        };
        let content = Rect {
            height: area.height.saturating_sub(1),
            ..area
        };
        let text_area = scroll::text_area(content);
        let text = self.paragraph(commit, &cx.editor.theme);
        self.scroll
            .update(content.height, text.line_count(text_area.width));
        let text = text.scroll((self.scroll.offset.min(u16::MAX as usize) as u16, 0));
        (&text).render(text_area, surface);
        self.rendered = Some((cx.editor.theme.revision(), text));
        self.scroll.render(content, surface, &cx.editor.theme);
        if area.height > 1 {
            let keys = &cx.config.component_keys.history_picker;
            surface.set_stringn(
                area.x,
                area.bottom() - 1,
                format!(
                    "{} open commit · {}/{} scroll",
                    action_hint(keys, &PickerAction::OpenPreview),
                    action_hint(keys, &PickerAction::PreviewScrollUp),
                    action_hint(keys, &PickerAction::PreviewScrollDown)
                ),
                area.width as usize,
                cx.editor.theme.get("ui.text.inactive"),
            );
        }
    }

    fn action(
        &mut self,
        item: Option<&Arc<CommitEntry>>,
        action: PreviewAction,
        cx: &mut Context,
    ) -> EventResult {
        self.update(item, Rect::default(), cx);
        self.opened = false;
        match action {
            PreviewAction::ScrollUp => self.scroll.scroll(false),
            PreviewAction::ScrollDown => self.scroll.scroll(true),
            PreviewAction::Open(action) => {
                if let Some(commit) = item
                    && self.reviews.is_some()
                {
                    let text = self.build_text(commit, &cx.editor.theme).to_string();
                    cx.editor.open_commit_preview(&commit.id, text, action);
                    self.opened = true;
                }
            }
            PreviewAction::Previous | PreviewAction::Next => {}
        }
        EventResult::Consumed(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{application::TestApplication, job::Jobs, ui::picker::test::application};
    use std::path::Path;

    #[test]
    fn messages_without_a_grammar_keep_literal_text_and_blank_lines() -> anyhow::Result<()> {
        let loader = Arc::new(Loader::new(
            toml::from_str("language = []")?,
            Default::default(),
        )?);
        let message = CommitMessage::new("Subject λ\r\n\r\nBody\t\u{1b}end\r\n".into(), loader);
        let lines = message.lines(&theme::Theme::default());
        assert_eq!(Text::from(lines).to_string(), "Subject λ\n\nBody    �end");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    #[cfg_attr(
        not(feature = "runtime-grammars"),
        ignore = "requires compiled runtime grammars; enable the runtime-grammars feature"
    )]
    async fn commit_highlights_messages_and_each_original_source_and_follows_theme_changes(
    ) -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        git(dir.path(), &["init"]);
        // The opening delimiter is outside the displayed hunk's context.
        let prefix = format!("/*\n{}", "unchanged\n".repeat(12));
        std::fs::write(dir.path().join("a.rs"), format!("{prefix}λ old\n*/\n"))?;
        std::fs::write(dir.path().join("b.rs"), "// second old\n")?;
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "Initial"]);
        std::fs::write(
            dir.path().join("a.rs"),
            format!("{prefix}*/\nlet value = \"λ new\";\n"),
        )?;
        std::fs::write(dir.path().join("b.rs"), "let second = \"second new\";\n")?;
        git(dir.path(), &["add", "."]);
        git(
            dir.path(),
            &[
                "commit",
                "-m",
                "feat(preview): highlight λ changes",
                "-m",
                "Body stays literal.",
            ],
        );
        let repo = GitRepository::discover(dir.path())?;
        let commit = repo
            .history_page(repo.history_cursor(None)?.unwrap())
            .await?
            .commits[0]
            .clone();
        let mut config = crate::config::Config::default();
        config.editor.lsp.enable = false;
        config.editor.files.watch.enable = false;
        config.editor.files.watch.watch_vcs = false;
        let loader = Loader::new(
            toml::from_str(
                r#"
            [[language]]
            name = "rust"
            scope = "source.rust"
            file-types = ["rs"]
            [[language]]
            name = "git-commit"
            scope = "git.commitmsg"
            grammar = "gitcommit"
            file-types = ["COMMIT_EDITMSG"]
        "#,
            )?,
            Default::default(),
        )?;
        let mut app = TestApplication::new(
            crate::args::Args::default(),
            config,
            loader,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        )?;
        app.editor.theme = theme::Theme::from(toml::from_str::<toml::Value>(
            r##"
            "ui.text" = "#ffffff"
            "comment" = "#00ff00"
            "string" = "#ff00ff"
            "markup.heading" = "#ffff00"
            "diff.plus" = "#0000ff"
            "diff.minus" = "#ff0000"
            "diff.plus.line" = { bg = "#001100" }
            "diff.minus.line" = { bg = "#110000" }
        "##,
        )?);
        let mut preview = CommitReviewPreview::new(repo);
        until_text(&mut preview, &commit, &mut app, "λ new").await?;
        until_message_highlight(&mut preview, &commit, &mut app).await?;
        let cell = |surface: &Buffer, needle: &str| {
            surface
                .content
                .chunks(120)
                .enumerate()
                .find_map(|(y, row)| {
                    let text: String = row.iter().map(|cell| cell.symbol()).collect();
                    text.find(needle).map(|byte| {
                        surface[(text[..byte].chars().count() as u16, y as u16)].clone()
                    })
                })
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        use view::graphics::Color;
        let surface = draw_frame(
            &mut preview,
            Some(&commit),
            &mut app,
            None,
            Rect::new(0, 0, 120, 80),
        );
        assert_eq!(
            cell(&surface, "highlight λ changes").fg,
            Color::Rgb(255, 255, 0)
        );
        assert_eq!(
            cell(&surface, "Body stays literal.").fg,
            Color::Rgb(255, 255, 255)
        );
        assert_eq!(cell(&surface, "λ old").fg, Color::Rgb(0, 255, 0));
        assert_eq!(cell(&surface, "λ new").fg, Color::Rgb(255, 0, 255));
        assert_eq!(cell(&surface, "second old").fg, Color::Rgb(0, 255, 0));
        assert_eq!(cell(&surface, "second new").fg, Color::Rgb(255, 0, 255));
        assert_eq!(cell(&surface, "λ old").bg, Color::Rgb(17, 0, 0));
        assert_eq!(cell(&surface, "λ new").bg, Color::Rgb(0, 17, 0));
        assert_eq!(cell(&surface, "-λ old").fg, Color::Rgb(255, 0, 0));
        assert_eq!(cell(&surface, "+let value").fg, Color::Rgb(0, 0, 255));

        app.editor.theme = theme::Theme::from(toml::from_str::<toml::Value>(
            r##"
            "ui.text" = "#ffffff"
            "comment" = "#0000ff"
            "constant" = "#ff0000"
            "string" = "#00ffff"
            "markup.heading" = "#ff8800"
        "##,
        )?);
        let surface = draw_frame(
            &mut preview,
            Some(&commit),
            &mut app,
            None,
            Rect::new(0, 0, 120, 80),
        );
        assert_eq!(
            cell(&surface, "highlight λ changes").fg,
            Color::Rgb(255, 136, 0)
        );
        assert_eq!(cell(&surface, "λ old").fg, Color::Rgb(0, 0, 255));
        assert_eq!(cell(&surface, "λ new").fg, Color::Rgb(0, 255, 255));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn source_budget_keeps_later_files_readable() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        git(dir.path(), &["init"]);
        // Each large comparison fits on its own, but both sources exceed the shared budget.
        // Keep the padding outside the displayed hunk's context.
        let padding = format!(
            "/*{}*/\n\n\n\n\n\n",
            "x".repeat(vcs::MAX_REVIEW_BYTES / 4 + 1024)
        );
        for name in ["a.rs", "b.rs"] {
            std::fs::write(dir.path().join(name), format!("{padding}let value = 1;\n"))?;
        }
        std::fs::write(dir.path().join("c.rs"), "let value = 1;\n")?;
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "Initial"]);
        for name in ["a.rs", "b.rs"] {
            std::fs::write(dir.path().join(name), format!("{padding}let value = 2;\n"))?;
        }
        std::fs::write(dir.path().join("c.rs"), "let value = 2;\n")?;
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "Update"]);
        let repo = GitRepository::discover(dir.path())?;
        let commit = repo
            .history_page(repo.history_cursor(None)?.unwrap())
            .await?
            .commits[0]
            .clone();
        let mut app = application(false)?;
        let mut preview = CommitReviewPreview::new(repo);
        until_text(&mut preview, &commit, &mut app, "let value = 2;").await?;
        let reviews = preview.reviews.as_ref().unwrap();
        assert_eq!(reviews.len(), 3);
        assert!(reviews.iter().all(|file| file
            .review
            .lines
            .iter()
            .any(|line| line.text == "let value = 2;")));
        assert_eq!(
            reviews.iter().filter(|file| file.source.is_none()).count(),
            1
        );
        assert!(reviews
            .iter()
            .find(|file| file.file.path() == Path::new("c.rs"))
            .unwrap()
            .source
            .is_some());
        assert!(
            reviews
                .iter()
                .filter_map(|file| file.source.as_ref())
                .map(|source| source.byte_len())
                .sum::<usize>()
                <= vcs::MAX_REVIEW_BYTES
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn busy_message_highlighter_does_not_delay_plain_message_or_patch() -> anyhow::Result<()>
    {
        let dir = tempfile::tempdir()?;
        git(dir.path(), &["init"]);
        std::fs::write(dir.path().join("a.txt"), "patch is ready\n")?;
        git(dir.path(), &["add", "."]);
        git(
            dir.path(),
            &["commit", "-m", "Subject", "-m", "Full message body"],
        );
        let repo = GitRepository::discover(dir.path())?;
        let commit = repo
            .history_page(repo.history_cursor(None)?.unwrap())
            .await?
            .commits[0]
            .clone();
        let mut app = application(false)?;
        let mut preview = CommitReviewPreview::new(repo);
        // Model a previous selection's blocking parse still holding the worker.
        let permit = preview.message_worker.clone().acquire_owned().await?;
        let text = until_text(&mut preview, &commit, &mut app, "patch is ready").await?;
        assert!(text.contains("Full message body"));
        assert!(!text.contains("Loading changes"));
        assert!(preview.highlighted_message.is_some());
        assert!(preview.message.as_ref().unwrap().syntax.is_none());

        drop(permit);
        until_message_highlight(&mut preview, &commit, &mut app).await?;
        let text = frame(&mut preview, Some(&commit), &mut app, None);
        assert!(text.contains("Full message body") && text.contains("patch is ready"));
        Ok(())
    }

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env_remove("GIT_DIR")
            .env("GIT_AUTHOR_NAME", "Preview Author")
            .env("GIT_AUTHOR_EMAIL", "preview@example.com")
            .env("GIT_COMMITTER_NAME", "Preview Author")
            .env("GIT_COMMITTER_EMAIL", "preview@example.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn frame(
        preview: &mut CommitReviewPreview,
        commit: Option<&Arc<CommitEntry>>,
        app: &mut TestApplication,
        action: Option<PreviewAction>,
    ) -> String {
        frame_in(preview, commit, app, action, Rect::new(0, 0, 100, 24))
    }

    fn frame_in(
        preview: &mut CommitReviewPreview,
        commit: Option<&Arc<CommitEntry>>,
        app: &mut TestApplication,
        action: Option<PreviewAction>,
        area: Rect,
    ) -> String {
        draw_frame(preview, commit, app, action, area)
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn draw_frame(
        preview: &mut CommitReviewPreview,
        commit: Option<&Arc<CommitEntry>>,
        app: &mut TestApplication,
        action: Option<PreviewAction>,
        area: Rect,
    ) -> Buffer {
        let config = crate::config::Config::default();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut jobs = Jobs::new();
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
            jobs: &mut jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: true,
        };
        let mut surface = Buffer::empty(area);
        preview.update(commit, area, &mut cx);
        if let Some(action) = action {
            preview.action(commit, action, &mut cx);
        }
        preview.render(commit, area, &mut surface, &mut cx.draw_context());
        surface
    }

    async fn until_message_highlight(
        preview: &mut CommitReviewPreview,
        commit: &Arc<CommitEntry>,
        app: &mut TestApplication,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            while preview.highlighted_message.is_some() {
                frame(preview, Some(commit), app, None);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn until_text(
        preview: &mut CommitReviewPreview,
        commit: &Arc<CommitEntry>,
        app: &mut TestApplication,
        needle: &str,
    ) -> anyhow::Result<String> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let text = frame(preview, Some(commit), app, None);
                if text.contains(needle) {
                    break text;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(Into::into)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn commit_preview_scrolls_all_files_and_preserves_normal_acceptance() -> anyhow::Result<()>
    {
        let dir = tempfile::tempdir()?;
        git(dir.path(), &["init"]);
        let root_alpha = format!("root alpha\n{}diff-tail\n", "diff line\n".repeat(40));
        std::fs::write(dir.path().join("a.txt"), &root_alpha)?;
        std::fs::write(dir.path().join("b.bin"), b"\0binary content")?;
        std::fs::write(dir.path().join("b.txt"), "root beta\n")?;
        git(dir.path(), &["add", "."]);
        git(
            dir.path(),
            &[
                "commit",
                "-m",
                "Root preview",
                "-m",
                "Full message body with 日本語.",
            ],
        );
        std::fs::write(
            dir.path().join("a.txt"),
            root_alpha.replacen("root alpha", "updated alpha", 1),
        )?;
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "Update preview"]);
        git(
            dir.path(),
            &["commit", "--allow-empty", "-m", "Empty preview"],
        );
        let repo = GitRepository::discover(dir.path())?;
        let commits = repo
            .history_page(repo.history_cursor(None)?.unwrap())
            .await?
            .commits;
        let mut app = application(false)?;
        let mut preview = CommitReviewPreview::new(repo);
        let root = &commits[2];
        let text = until_text(&mut preview, root, &mut app, "root alpha").await?;
        assert!(text.contains("Root preview") && text.contains("Preview Author"));
        assert!(
            text.contains("Full message body with ") && text.contains('日'),
            "{text}"
        );
        assert!(text.contains("a.txt"));
        let mut scrolled = String::new();
        let mut seen = text;
        for _ in 0..20 {
            scrolled = frame(
                &mut preview,
                Some(root),
                &mut app,
                Some(PreviewAction::ScrollDown),
            );
            seen.push_str(&scrolled);
        }
        assert!(
            seen.contains("diff-tail") && seen.contains("root beta"),
            "scrolling alone must reach every file"
        );
        assert!(seen.contains("b.bin") && seen.contains("Binary file"));
        for _ in 0..20 {
            scrolled = frame(
                &mut preview,
                Some(root),
                &mut app,
                Some(PreviewAction::ScrollUp),
            );
        }
        assert!(scrolled.contains("Full message body") && scrolled.contains("root alpha"));
        assert!(
            !preview.open_on_accept(root),
            "Enter must still browse changed files"
        );
        let text = until_text(&mut preview, &commits[1], &mut app, "updated alpha").await?;
        assert!(text.contains("root alpha") && !text.contains("root beta"));
        assert!(!text.contains("Full message body"));
        until_text(&mut preview, &commits[0], &mut app, "No changed files").await?;
        until_text(&mut preview, root, &mut app, "root alpha").await?;
        frame(
            &mut preview,
            Some(root),
            &mut app,
            Some(PreviewAction::Open(view::editor::Action::Replace)),
        );
        assert!(
            preview.close_on_open(),
            "Alt-o must open the combined commit"
        );
        let (_, doc) = view::current_ref!(&app.editor);
        assert!(doc.is_readonly() && doc.path().is_none());
        let opened = doc.text().to_string();
        assert!(opened.contains("root alpha") && opened.contains("root beta"));
        assert!(opened.contains("--- /dev/null") && opened.contains("+++ b/b.txt"));
        assert!(opened.contains("b.bin") && opened.contains("Binary file"));
        assert!(opened.contains("Full message body with 日本語."));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn long_messages_wrap_and_scroll_even_without_changed_files() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        git(dir.path(), &["init"]);
        let subject = format!("{} subject-tail", "long subject ".repeat(12));
        let body = (0..40)
            .map(|i| format!("body line {i:02}\n"))
            .collect::<String>();
        git(
            dir.path(),
            &["commit", "--allow-empty", "-m", &subject, "-m", &body],
        );
        let repo = GitRepository::discover(dir.path())?;
        let commits = repo
            .history_page(repo.history_cursor(None)?.unwrap())
            .await?
            .commits;
        let commit = &commits[0];
        let mut app = application(false)?;
        let mut preview = CommitReviewPreview::new(repo);
        until_text(&mut preview, commit, &mut app, "body line 00").await?;
        let area = Rect::new(0, 0, 30, 12);
        let mut seen = frame_in(&mut preview, Some(commit), &mut app, None, area);
        for _ in 0..70 {
            seen.push_str(&frame_in(
                &mut preview,
                Some(commit),
                &mut app,
                Some(PreviewAction::ScrollDown),
                area,
            ));
        }
        assert!(seen.contains("subject-tail"), "long subjects must wrap");
        for i in 0..40 {
            assert!(
                seen.contains(&format!("body line {i:02}")),
                "missing body line {i}"
            );
        }
        for _ in 0..70 {
            frame_in(
                &mut preview,
                Some(commit),
                &mut app,
                Some(PreviewAction::ScrollUp),
                area,
            );
        }
        assert!(frame_in(&mut preview, Some(commit), &mut app, None, area).contains("long subject"));
        for height in 0..6 {
            frame_in(
                &mut preview,
                Some(commit),
                &mut app,
                None,
                Rect::new(0, 0, 1, height),
            );
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn changing_or_hiding_commit_preview_discards_pending_reads() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        git(dir.path(), &["init"]);
        git(dir.path(), &["commit", "--allow-empty", "-m", "First"]);
        git(dir.path(), &["commit", "--allow-empty", "-m", "Second"]);
        let repo = GitRepository::discover(dir.path())?;
        let commits = repo
            .history_page(repo.history_cursor(None)?.unwrap())
            .await?
            .commits;
        let mut app = application(false)?;
        let mut preview = CommitReviewPreview::new(repo);
        frame(&mut preview, Some(&commits[0]), &mut app, None);
        assert!(preview.details.is_some());
        frame(&mut preview, Some(&commits[1]), &mut app, None);
        assert_eq!(preview.selected.as_ref(), Some(&commits[1].id));
        preview.cancel();
        assert!(
            preview.details.is_none()
                && preview.pending.is_none()
                && preview.reviews.is_none()
                && preview.selected.is_none()
        );
        frame(&mut preview, Some(&commits[0]), &mut app, None);
        frame(&mut preview, None, &mut app, None);
        assert!(
            preview.details.is_none()
                && preview.pending.is_none()
                && preview.highlighted_message.is_none()
                && preview.selected.is_none()
        );
        until_text(&mut preview, &commits[1], &mut app, "No changed files").await?;
        Ok(())
    }
}
