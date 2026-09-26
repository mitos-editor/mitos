//! Path styling shared by feature-owned pickers.

use std::path::Path;
use tui::{
    text::{Line, Span},
    widgets::Cell,
};
use view::{document::SCRATCH_BUFFER_NAME, icons::ICONS, theme::Style, Editor};

pub(super) struct PathStyleConfig {
    theme: std::sync::Arc<view::Theme>,
    icons: bool,
    directory_style: Style,
    number_style: Style,
    colon_style: Style,
}

impl PathStyleConfig {
    pub(super) fn new(editor: &Editor) -> Self {
        let theme = &editor.theme;
        Self {
            theme: std::sync::Arc::new(theme.clone()),
            icons: editor.config().icons,
            directory_style: theme.get("ui.text.directory"),
            number_style: theme.get("constant.numeric.integer"),
            colon_style: theme.get("punctuation"),
        }
    }

    pub(super) fn stylize<'a>(&self, path: Option<&'a Path>, line: Option<usize>) -> Cell<'a> {
        let mut spans = Vec::new();
        if let Some(path) = path {
            if self.icons {
                let icons = ICONS.load();
                if let Some(file) = icons.fs().file() {
                    spans.push(Span::from(
                        file.get_with_style_or_default(path, self.theme.as_ref()),
                    ));
                }
            }
            let directories = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| format!("{}{}", p.display(), std::path::MAIN_SEPARATOR))
                .unwrap_or_default();
            spans.push(Span::styled(directories, self.directory_style));
        }
        let filename = path.as_ref().map_or(SCRATCH_BUFFER_NAME.into(), |path| {
            path.file_name()
                .expect("all document names are normalized (can't end in `..`)")
                .to_string_lossy()
        });
        spans.push(Span::raw(filename));
        if let Some(line) = line {
            spans.extend([
                Span::styled(":", self.colon_style),
                Span::styled((line + 1).to_string(), self.number_style),
            ]);
        }

        Cell::from(Line::from(spans))
    }
}
