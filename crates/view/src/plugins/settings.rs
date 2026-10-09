//! Small owner stacks over the current native configuration, never copied configs.
use super::*;
use crate::Theme;
use plugin_api::{
    editor::{SettingValue, SettingsScope},
    ui::UiOwner,
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Setting {
    AutoFormat,
    SoftWrap,
    CursorLine,
}
#[derive(Default)]
pub(crate) struct Settings {
    sequence: u64,
    values: BTreeMap<(Option<u64>, Setting, UiOwner), (u64, bool)>,
    themes: BTreeMap<UiOwner, (u64, Theme)>,
    baseline: Option<Theme>,
    applied: bool,
}
pub(crate) type SettingsHandle = Arc<Mutex<Settings>>;

impl Settings {
    pub(crate) fn value(&self, document: Option<u64>, setting: Setting) -> Option<bool> {
        let latest = |scope| {
            self.values
                .iter()
                .filter(|((doc, kind, _), _)| *doc == scope && *kind == setting)
                .max_by_key(|(_, (sequence, _))| *sequence)
                .map(|(_, (_, value))| *value)
        };
        document
            .and_then(|doc| latest(Some(doc)))
            .or_else(|| latest(None))
    }
    fn next(&mut self) -> Result<u64, ServiceError> {
        self.sequence = self.sequence.checked_add(1).ok_or_else(|| {
            ServiceError::new(ErrorCode::HostFailure, "plugin setting sequence exhausted")
        })?;
        Ok(self.sequence)
    }
}

impl Editor {
    pub(crate) fn plugin_settings_handle(&self) -> std::sync::Weak<Mutex<Settings>> {
        Arc::downgrade(&self.plugins.settings)
    }
    pub(super) fn plugin_setting_scope(
        &self,
        scope: SettingsScope,
    ) -> Result<Option<u64>, ServiceError> {
        match scope {
            SettingsScope::Editor => Ok(None),
            SettingsScope::Document { target } => {
                self.plugin_document(target.document, target.version)
                    .map_err(|error| ServiceError::new(ErrorCode::StaleState, error.to_string()))?;
                Ok(Some(target.document))
            }
        }
    }
    pub(super) fn plugin_read_settings(
        &self,
        scope: SettingsScope,
    ) -> Result<Vec<SettingValue>, ServiceError> {
        let document = self.plugin_setting_scope(scope)?;
        let state = self.plugins.settings.lock();
        let config = self.config();
        let doc =
            document.and_then(|id| self.documents.values().find(|doc| doc.id().as_u64() == id));
        let auto_format = state
            .value(document, Setting::AutoFormat)
            .unwrap_or_else(|| {
                doc.and_then(|doc| doc.language_config())
                    .and_then(|lang| lang.auto_format)
                    .unwrap_or(config.auto_format)
            });
        let soft_wrap = state.value(document, Setting::SoftWrap).unwrap_or_else(|| {
            doc.and_then(|doc| doc.language_config())
                .and_then(|lang| lang.soft_wrap.as_ref())
                .and_then(|wrap| wrap.enable)
                .or(config.soft_wrap.enable)
                .unwrap_or(false)
        });
        let cursorline = state
            .value(document, Setting::CursorLine)
            .unwrap_or(config.cursorline);
        let mut values = vec![
            SettingValue::AutoFormat(auto_format),
            SettingValue::SoftWrap(soft_wrap),
            SettingValue::CursorLine(cursorline),
        ];
        if document.is_none() {
            values.push(SettingValue::Theme(self.theme.name().to_owned()));
        }
        Ok(values)
    }
    pub(super) fn plugin_override_setting(
        &mut self,
        owner: UiOwner,
        scope: SettingsScope,
        value: SettingValue,
        theme: Option<Theme>,
    ) -> Result<(), ServiceError> {
        let document = self.plugin_setting_scope(scope)?;
        let mut state = self.plugins.settings.lock();
        let setting = match &value {
            SettingValue::AutoFormat(_) => Some(Setting::AutoFormat),
            SettingValue::SoftWrap(_) => Some(Setting::SoftWrap),
            SettingValue::CursorLine(_) => Some(Setting::CursorLine),
            SettingValue::Theme(_) => None,
        };
        let replacing = setting.map_or_else(
            || state.themes.contains_key(&owner),
            |setting| {
                state
                    .values
                    .contains_key(&(document, setting, owner.clone()))
            },
        );
        if !replacing
            && (state.values.len() + state.themes.len() >= 256
                || state
                    .values
                    .keys()
                    .filter(|(_, _, candidate)| *candidate == owner)
                    .count()
                    + usize::from(state.themes.contains_key(&owner))
                    >= 64)
        {
            return Err(ServiceError::new(
                ErrorCode::ResourceExhausted,
                "plugin setting override limit exceeded",
            ));
        }
        let sequence = state.next()?;
        match value {
            SettingValue::AutoFormat(value) => {
                state
                    .values
                    .insert((document, Setting::AutoFormat, owner), (sequence, value));
            }
            SettingValue::SoftWrap(value) => {
                state
                    .values
                    .insert((document, Setting::SoftWrap, owner), (sequence, value));
            }
            SettingValue::CursorLine(value) => {
                state
                    .values
                    .insert((document, Setting::CursorLine, owner), (sequence, value));
            }
            SettingValue::Theme(_) => {
                let theme = theme.ok_or_else(|| {
                    ServiceError::new(ErrorCode::HostFailure, "theme was not prepared")
                })?;
                if state.baseline.is_none() || !state.applied {
                    state.baseline = Some(self.theme.clone());
                }
                state.themes.insert(owner, (sequence, theme.clone()));
                state.applied = true;
                drop(state);
                self.apply_owned_plugin_theme(theme)?;
                event::request_redraw();
                return Ok(());
            }
        }
        drop(state);
        event::request_redraw();
        Ok(())
    }
    fn apply_owned_plugin_theme(&mut self, theme: Theme) -> Result<(), ServiceError> {
        if theme.find_highlight_exact("ui.selection").is_none() {
            return Err(ServiceError::new(
                ErrorCode::InvalidRequest,
                "theme requires ui.selection",
            ));
        }
        self.syn_loader.load().set_scopes(theme.scopes().to_vec());
        self.last_theme = None;
        self.theme = theme;
        event::request_redraw();
        Ok(())
    }
    pub(super) fn plugin_clear_settings(
        &mut self,
        owner: &UiOwner,
        scope: SettingsScope,
    ) -> Result<(), ServiceError> {
        let document = self.plugin_setting_scope(scope)?;
        let mut state = self.plugins.settings.lock();
        state
            .values
            .retain(|(doc, _, candidate), _| *doc != document || candidate != owner);
        let theme = if document.is_none() && state.themes.remove(owner).is_some() && state.applied {
            state
                .themes
                .values()
                .max_by_key(|(sequence, _)| *sequence)
                .map(|(_, theme)| theme.clone())
                .or_else(|| state.baseline.take())
        } else {
            None
        };
        if state.themes.is_empty() {
            state.applied = false;
            state.baseline = None;
        }
        drop(state);
        if let Some(theme) = theme {
            self.apply_owned_plugin_theme(theme)?;
        }
        event::request_redraw();
        Ok(())
    }
    /// Native theme changes become the latest baseline. A later unload cannot
    /// restore an older theme over a user change, including a config reload.
    pub(crate) fn plugin_native_theme_changed(&mut self) {
        let mut state = self.plugins.settings.lock();
        if !state.themes.is_empty() {
            state.baseline = Some(self.theme.clone());
            state.applied = false;
        }
    }
    pub(super) fn prune_plugin_settings(&mut self) {
        let mut state = self.plugins.settings.lock();
        state
            .values
            .retain(|(_, _, owner), _| self.plugin_owner_is_current(owner));
        let previous = state.themes.len();
        state
            .themes
            .retain(|owner, _| self.plugin_owner_is_current(owner));
        let theme = if previous != state.themes.len() && state.applied {
            state
                .themes
                .values()
                .max_by_key(|(sequence, _)| *sequence)
                .map(|(_, theme)| theme.clone())
                .or_else(|| state.baseline.take())
        } else {
            None
        };
        if state.themes.is_empty() {
            state.applied = false;
            state.baseline = None;
        }
        drop(state);
        if let Some(theme) = theme {
            let _ = self.apply_owned_plugin_theme(theme);
        }
    }
    pub(super) fn clear_all_plugin_settings(&mut self) {
        let theme = {
            let mut state = self.plugins.settings.lock();
            let theme = state.applied.then(|| state.baseline.take()).flatten();
            *state = Settings::default();
            theme
        };
        if let Some(theme) = theme {
            let _ = self.apply_owned_plugin_theme(theme);
        }
        event::request_redraw();
    }
    pub(crate) fn clear_plugin_document_settings(&mut self, document: u64) {
        self.plugins
            .settings
            .lock()
            .values
            .retain(|(doc, _, _), _| *doc != Some(document));
    }
}

impl Document {
    fn plugin_setting(&self, setting: Setting) -> Option<bool> {
        self.plugin_settings
            .as_ref()?
            .upgrade()?
            .lock()
            .value(Some(self.id().as_u64()), setting)
    }
    pub fn plugin_cursorline(&self, baseline: bool) -> bool {
        self.plugin_setting(Setting::CursorLine).unwrap_or(baseline)
    }
    pub(crate) fn plugin_auto_format(&self) -> Option<bool> {
        self.plugin_setting(Setting::AutoFormat)
    }
    pub(crate) fn plugin_soft_wrap(&self) -> Option<bool> {
        self.plugin_setting(Setting::SoftWrap)
    }
}
