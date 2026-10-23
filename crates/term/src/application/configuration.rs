//! Apply live configuration and choose a theme supported by this terminal.
use super::{Application, Terminal};
use crate::config::{Config, EditorSettings};
use anyhow::Error;
use std::sync::Arc;
use tui::backend::BackendExt;
use view::{editor::ConfigEvent, Editor};

impl Application {
    pub fn handle_config_events(&mut self, config_event: ConfigEvent) {
        let old_editor_config = self.editor.config();

        match config_event {
            ConfigEvent::Refresh => {
                self.refresh_config();
                if let Err(error) = self.editor.reload_snippets() {
                    self.editor.set_error(|| format!("{error:#}"));
                }
            }

            ConfigEvent::Update(editor_config) => {
                let terminal = self.config.load().terminal.clone();
                self.apply_editor_settings(EditorSettings {
                    editor: *editor_config,
                    terminal,
                });
            }
            ConfigEvent::ThemeChanged => {
                let _ = self.terminal.backend_mut().set_background_color(
                    self.editor
                        .theme
                        .try_get_exact("ui.background")
                        .and_then(|style| style.bg),
                );
                return;
            }
        }

        self.editor.refresh_config(&old_editor_config);
    }

    pub(super) fn apply_editor_settings(&mut self, settings: EditorSettings) {
        let mut app_config = (**self.config.load()).clone();
        app_config.editor = settings.editor;
        app_config.terminal = settings.terminal;
        if let Err(err) = self
            .terminal
            .backend_mut()
            .reconfigure(terminal_config(&app_config))
        {
            self.editor.set_error(|| err.to_string());
        }
        self.config.store(Arc::new(app_config));
    }

    pub(super) fn refresh_config(&mut self) {
        let mut refresh_config = || -> Result<(), Error> {
            let default_config = Config::load_default()
                .map_err(|err| anyhow::anyhow!("Failed to load config: {}", err))?;

            let lang_loader = self.editor.load_language_config(&default_config.editor)?;
            let theme = Self::configured_theme(
                &self.editor,
                &default_config,
                &self.terminal,
                self.theme_mode,
            );
            let _ = self.editor.apply_language_config(lang_loader, theme);

            self.terminal
                .backend_mut()
                .reconfigure(terminal_config(&default_config))?;
            // Store new config
            self.config.store(Arc::new(default_config));
            Ok(())
        };

        match refresh_config() {
            Ok(()) => self.editor.set_status("Config refreshed"),
            Err(err) => {
                self.editor.set_error(|| err.to_string());
            }
        }
    }

    /// Choose a configured theme compatible with this frontend.
    pub(super) fn configured_theme(
        editor: &Editor,
        config: &Config,
        terminal: &Terminal,
        mode: Option<theme::Mode>,
    ) -> theme::Theme {
        let true_color = terminal.backend().supports_true_color()
            || config.terminal.true_color
            || crate::true_color();
        let theme_config = config.theme.clone().unwrap_or_default();
        let name = theme_config.choose(mode);
        editor
            .theme_loader
            .load(name)
            .map_err(|e| {
                log::warn!("failed to load theme `{}` - {}", name, e);
                e
            })
            .ok()
            .filter(|theme| {
                let colors_ok = true_color || theme.is_16_color();
                if !colors_ok {
                    log::warn!(
                        "loaded theme `{}` but cannot use it because true color \
                        support is not enabled",
                        theme.name()
                    );
                }
                colors_ok
            })
            .unwrap_or_else(|| editor.theme_loader.default_theme())
    }
}

pub(super) fn terminal_config(config: &Config) -> tui::terminal::Config {
    tui::terminal::Config {
        enable_mouse_capture: config.editor.mouse.enable,
        force_enable_extended_underlines: config.terminal.undercurl,
        kitty_keyboard_protocol: config.terminal.keyboard.kitty_protocol,
    }
}
