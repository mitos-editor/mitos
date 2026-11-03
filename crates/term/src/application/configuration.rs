//! Apply live configuration and choose a theme supported by this terminal.
use super::App;
use crate::config::Config;
use anyhow::Error;
use std::sync::Arc;
use view::{editor::ConfigEvent, Editor};

impl<B: tui::backend::Backend + tui::backend::BackendExt> App<B>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    pub fn handle_config_events(&mut self, event: ConfigEvent) {
        self.apply_config_update(crate::config::ConfigEvent::Shared(event));
    }

    /// Apply a request and report whether settings require an immediate frame.
    pub fn apply_config_update(&mut self, event: crate::config::ConfigEvent) -> bool {
        use crate::config::ConfigEvent as Request;
        let old_editor_config = self.editor.config();
        match event {
            Request::Shared(ConfigEvent::Refresh) => {
                self.refresh_config();
                if let Err(error) = self.editor.reload_snippets() {
                    self.editor.set_error(|| format!("{error:#}"));
                }
            }
            Request::Shared(ConfigEvent::ThemeChanged) => {
                let _ = self.terminal.backend_mut().set_background_color(
                    self.editor
                        .theme
                        .try_get_exact("ui.background")
                        .and_then(|style| style.bg),
                );
                self.request_redraw();
                return false;
            }
            Request::Update(settings) => {
                self.apply_settings_update(settings.editor, Some(settings.terminal));
            }
            Request::UpdateEditor(editor) => self.apply_settings_update(*editor, None),
        }

        self.editor.refresh_config(&old_editor_config);
        true
    }

    /// Take the next request when driving this application manually. Apply it with
    /// `apply_config_update` before accepting further input.
    pub fn take_config_request(&mut self) -> Option<crate::config::ConfigEvent> {
        self.config_updates.1.try_recv().ok()
    }

    /// Apply settings and shared effects before another input can observe the old snapshot.
    pub(super) fn apply_pending_config_updates(&mut self) -> bool {
        let mut updated = false;
        while let Some(event) = self.take_config_request() {
            updated |= self.apply_config_update(event);
        }
        updated
    }

    fn apply_settings_update(
        &mut self,
        editor: editor_config::Config,
        terminal: Option<crate::config::TerminalConfig>,
    ) {
        let mut config = (**self.config.load()).clone();
        config.editor = editor;
        if let Some(terminal) = terminal {
            config.terminal = terminal;
        }
        if let Err(error) = self
            .terminal
            .backend_mut()
            .reconfigure(terminal_config(&config))
        {
            self.editor.set_error(|| error.to_string());
        }
        self.config.store(Arc::new(config));
    }

    pub(super) fn refresh_config(&mut self) {
        let mut refresh_config = || -> Result<(), Error> {
            let workspace = workspace::ResolvedWorkspace::current();
            let default_config = Config::load_for(&workspace)
                .map_err(|err| anyhow::anyhow!("Failed to load config: {}", err))?;

            let lang_loader = self
                .editor
                .load_language_config(&default_config.editor, workspace)?;
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
        terminal: &tui::Terminal<B>,
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

#[cfg(test)]
mod tests {
    use crate::application::TestApplication;
    use crate::ui::picker::test::application;

    async fn keys(app: &mut TestApplication, input: &str) -> anyhow::Result<()> {
        for key in ui_core::input::parse_macro(input)? {
            #[cfg(not(windows))]
            let event = termina::event::Event::Key(tui::backend::native::adapt(key));
            #[cfg(windows)]
            let event = crossterm::event::Event::Key(tui::backend::native::adapt(key));
            app.handle_terminal_events(Ok(event)).await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn shell_settings_apply_before_the_next_terminal_command() -> anyhow::Result<()> {
        let mut app = application(false)?;
        for (input, expected) in [
            (
                r#":set shell ["/mitos-missing-shell"]<ret>:get shell<ret>"#,
                r#"["/mitos-missing-shell"]"#,
            ),
            (
                r#":set shell ["sh", "-c"]<ret>:get shell<ret>"#,
                r#"["sh","-c"]"#,
            ),
        ] {
            keys(&mut app, input).await?;
            assert_eq!(app.editor.get_status().unwrap().0.as_ref(), expected);
        }

        // Updates completed by asynchronous commands must also precede pending input.
        keys(&mut app, ":get shell").await?;
        let mut settings = app.config.load().editor_settings();
        settings.editor.shell = vec!["/mitos-missing-shell".into()];
        app.config_updates
            .0
            .send(crate::config::ConfigEvent::Update(Box::new(settings)))?;
        keys(&mut app, "<ret>").await?;
        assert_eq!(
            app.editor.get_status().unwrap().0.as_ref(),
            r#"["/mitos-missing-shell"]"#
        );
        Ok(())
    }
}
