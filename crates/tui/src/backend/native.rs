//! Platform adapters for backend-neutral UI values.
//!
//! All native terminal dependencies stay at this boundary; shared input and
//! style values do not depend on a backend or a platform feature.

use ui_core::{
    graphics::UnderlineStyle,
    input::{Event, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    keyboard::{KeyCode, KeyModifiers, MediaKeyCode, ModifierKeyCode},
};

#[cfg(windows)]
use crossterm::event as platform;
#[cfg(windows)]
use platform::{
    KeyCode::{Esc as EscapeKey, F as FunctionKey},
    KeyModifiers as NativeModifiers,
};
#[cfg(not(windows))]
use platform::{
    KeyCode::{Escape as EscapeKey, Function as FunctionKey},
    Modifiers as NativeModifiers,
};
#[cfg(not(windows))]
use termina::event as platform;

/// A conversion owned by the terminal backend rather than by portable UI types.
pub trait AdaptFrom<T> {
    fn adapt_from(value: T) -> Self;
}

/// Convert an input, style, or capability value at the native terminal boundary.
pub fn adapt<T, U: AdaptFrom<T>>(value: T) -> U {
    U::adapt_from(value)
}

impl AdaptFrom<KeyModifiers> for NativeModifiers {
    fn adapt_from(key_modifiers: KeyModifiers) -> Self {
        type CKeyModifiers = NativeModifiers;

        let mut result = CKeyModifiers::NONE;

        if key_modifiers.contains(KeyModifiers::SHIFT) {
            result.insert(CKeyModifiers::SHIFT);
        }
        if key_modifiers.contains(KeyModifiers::CONTROL) {
            result.insert(CKeyModifiers::CONTROL);
        }
        if key_modifiers.contains(KeyModifiers::ALT) {
            result.insert(CKeyModifiers::ALT);
        }
        if key_modifiers.contains(KeyModifiers::SUPER) {
            result.insert(CKeyModifiers::SUPER);
        }

        result
    }
}

impl AdaptFrom<NativeModifiers> for KeyModifiers {
    fn adapt_from(val: NativeModifiers) -> Self {
        type CKeyModifiers = NativeModifiers;

        let mut result = KeyModifiers::NONE;

        if val.contains(CKeyModifiers::SHIFT) {
            result.insert(KeyModifiers::SHIFT);
        }
        if val.contains(CKeyModifiers::CONTROL) {
            result.insert(KeyModifiers::CONTROL);
        }
        if val.contains(CKeyModifiers::ALT) {
            result.insert(KeyModifiers::ALT);
        }
        if val.contains(CKeyModifiers::SUPER) {
            result.insert(KeyModifiers::SUPER);
        }

        result
    }
}

impl AdaptFrom<MediaKeyCode> for platform::MediaKeyCode {
    fn adapt_from(media_key_code: MediaKeyCode) -> Self {
        use platform::MediaKeyCode as CMediaKeyCode;

        match media_key_code {
            MediaKeyCode::Play => CMediaKeyCode::Play,
            MediaKeyCode::Pause => CMediaKeyCode::Pause,
            MediaKeyCode::PlayPause => CMediaKeyCode::PlayPause,
            MediaKeyCode::Reverse => CMediaKeyCode::Reverse,
            MediaKeyCode::Stop => CMediaKeyCode::Stop,
            MediaKeyCode::FastForward => CMediaKeyCode::FastForward,
            MediaKeyCode::Rewind => CMediaKeyCode::Rewind,
            MediaKeyCode::TrackNext => CMediaKeyCode::TrackNext,
            MediaKeyCode::TrackPrevious => CMediaKeyCode::TrackPrevious,
            MediaKeyCode::Record => CMediaKeyCode::Record,
            MediaKeyCode::LowerVolume => CMediaKeyCode::LowerVolume,
            MediaKeyCode::RaiseVolume => CMediaKeyCode::RaiseVolume,
            MediaKeyCode::MuteVolume => CMediaKeyCode::MuteVolume,
        }
    }
}

impl AdaptFrom<platform::MediaKeyCode> for MediaKeyCode {
    fn adapt_from(val: platform::MediaKeyCode) -> Self {
        use platform::MediaKeyCode as CMediaKeyCode;

        match val {
            CMediaKeyCode::Play => MediaKeyCode::Play,
            CMediaKeyCode::Pause => MediaKeyCode::Pause,
            CMediaKeyCode::PlayPause => MediaKeyCode::PlayPause,
            CMediaKeyCode::Reverse => MediaKeyCode::Reverse,
            CMediaKeyCode::Stop => MediaKeyCode::Stop,
            CMediaKeyCode::FastForward => MediaKeyCode::FastForward,
            CMediaKeyCode::Rewind => MediaKeyCode::Rewind,
            CMediaKeyCode::TrackNext => MediaKeyCode::TrackNext,
            CMediaKeyCode::TrackPrevious => MediaKeyCode::TrackPrevious,
            CMediaKeyCode::Record => MediaKeyCode::Record,
            CMediaKeyCode::LowerVolume => MediaKeyCode::LowerVolume,
            CMediaKeyCode::RaiseVolume => MediaKeyCode::RaiseVolume,
            CMediaKeyCode::MuteVolume => MediaKeyCode::MuteVolume,
        }
    }
}

impl AdaptFrom<ModifierKeyCode> for platform::ModifierKeyCode {
    fn adapt_from(modifier_key_code: ModifierKeyCode) -> Self {
        use platform::ModifierKeyCode as CModifierKeyCode;

        match modifier_key_code {
            ModifierKeyCode::LeftShift => CModifierKeyCode::LeftShift,
            ModifierKeyCode::LeftControl => CModifierKeyCode::LeftControl,
            ModifierKeyCode::LeftAlt => CModifierKeyCode::LeftAlt,
            ModifierKeyCode::LeftSuper => CModifierKeyCode::LeftSuper,
            ModifierKeyCode::LeftHyper => CModifierKeyCode::LeftHyper,
            ModifierKeyCode::LeftMeta => CModifierKeyCode::LeftMeta,
            ModifierKeyCode::RightShift => CModifierKeyCode::RightShift,
            ModifierKeyCode::RightControl => CModifierKeyCode::RightControl,
            ModifierKeyCode::RightAlt => CModifierKeyCode::RightAlt,
            ModifierKeyCode::RightSuper => CModifierKeyCode::RightSuper,
            ModifierKeyCode::RightHyper => CModifierKeyCode::RightHyper,
            ModifierKeyCode::RightMeta => CModifierKeyCode::RightMeta,
            ModifierKeyCode::IsoLevel3Shift => CModifierKeyCode::IsoLevel3Shift,
            ModifierKeyCode::IsoLevel5Shift => CModifierKeyCode::IsoLevel5Shift,
        }
    }
}

impl AdaptFrom<platform::ModifierKeyCode> for ModifierKeyCode {
    fn adapt_from(val: platform::ModifierKeyCode) -> Self {
        use platform::ModifierKeyCode as CModifierKeyCode;

        match val {
            CModifierKeyCode::LeftShift => ModifierKeyCode::LeftShift,
            CModifierKeyCode::LeftControl => ModifierKeyCode::LeftControl,
            CModifierKeyCode::LeftAlt => ModifierKeyCode::LeftAlt,
            CModifierKeyCode::LeftSuper => ModifierKeyCode::LeftSuper,
            CModifierKeyCode::LeftHyper => ModifierKeyCode::LeftHyper,
            CModifierKeyCode::LeftMeta => ModifierKeyCode::LeftMeta,
            CModifierKeyCode::RightShift => ModifierKeyCode::RightShift,
            CModifierKeyCode::RightControl => ModifierKeyCode::RightControl,
            CModifierKeyCode::RightAlt => ModifierKeyCode::RightAlt,
            CModifierKeyCode::RightSuper => ModifierKeyCode::RightSuper,
            CModifierKeyCode::RightHyper => ModifierKeyCode::RightHyper,
            CModifierKeyCode::RightMeta => ModifierKeyCode::RightMeta,
            CModifierKeyCode::IsoLevel3Shift => ModifierKeyCode::IsoLevel3Shift,
            CModifierKeyCode::IsoLevel5Shift => ModifierKeyCode::IsoLevel5Shift,
        }
    }
}

impl AdaptFrom<KeyCode> for platform::KeyCode {
    fn adapt_from(key_code: KeyCode) -> Self {
        use platform::KeyCode as CKeyCode;

        match key_code {
            KeyCode::Backspace => CKeyCode::Backspace,
            KeyCode::Enter => CKeyCode::Enter,
            KeyCode::Left => CKeyCode::Left,
            KeyCode::Right => CKeyCode::Right,
            KeyCode::Up => CKeyCode::Up,
            KeyCode::Down => CKeyCode::Down,
            KeyCode::Home => CKeyCode::Home,
            KeyCode::End => CKeyCode::End,
            KeyCode::PageUp => CKeyCode::PageUp,
            KeyCode::PageDown => CKeyCode::PageDown,
            KeyCode::Tab => CKeyCode::Tab,
            KeyCode::Delete => CKeyCode::Delete,
            KeyCode::Insert => CKeyCode::Insert,
            KeyCode::F(f_number) => FunctionKey(f_number),
            KeyCode::Char(character) => CKeyCode::Char(character),
            KeyCode::Null => CKeyCode::Null,
            KeyCode::Esc => EscapeKey,
            KeyCode::CapsLock => CKeyCode::CapsLock,
            KeyCode::ScrollLock => CKeyCode::ScrollLock,
            KeyCode::NumLock => CKeyCode::NumLock,
            KeyCode::PrintScreen => CKeyCode::PrintScreen,
            KeyCode::Pause => CKeyCode::Pause,
            KeyCode::Menu => CKeyCode::Menu,
            KeyCode::KeypadBegin => CKeyCode::KeypadBegin,
            KeyCode::Media(media_key_code) => CKeyCode::Media(adapt(media_key_code)),
            KeyCode::Modifier(modifier_key_code) => CKeyCode::Modifier(adapt(modifier_key_code)),
        }
    }
}

impl AdaptFrom<platform::KeyCode> for KeyCode {
    fn adapt_from(val: platform::KeyCode) -> Self {
        use platform::KeyCode as CKeyCode;

        match val {
            CKeyCode::Backspace => KeyCode::Backspace,
            CKeyCode::Enter => KeyCode::Enter,
            CKeyCode::Left => KeyCode::Left,
            CKeyCode::Right => KeyCode::Right,
            CKeyCode::Up => KeyCode::Up,
            CKeyCode::Down => KeyCode::Down,
            CKeyCode::Home => KeyCode::Home,
            CKeyCode::End => KeyCode::End,
            CKeyCode::PageUp => KeyCode::PageUp,
            CKeyCode::PageDown => KeyCode::PageDown,
            CKeyCode::Tab => KeyCode::Tab,
            CKeyCode::BackTab => unreachable!("BackTab should have been handled on KeyEvent level"),
            CKeyCode::Delete => KeyCode::Delete,
            CKeyCode::Insert => KeyCode::Insert,
            FunctionKey(f_number) => KeyCode::F(f_number),
            CKeyCode::Char(character) => KeyCode::Char(character),
            CKeyCode::Null => KeyCode::Null,
            EscapeKey => KeyCode::Esc,
            CKeyCode::CapsLock => KeyCode::CapsLock,
            CKeyCode::ScrollLock => KeyCode::ScrollLock,
            CKeyCode::NumLock => KeyCode::NumLock,
            CKeyCode::PrintScreen => KeyCode::PrintScreen,
            CKeyCode::Pause => KeyCode::Pause,
            CKeyCode::Menu => KeyCode::Menu,
            CKeyCode::KeypadBegin => KeyCode::KeypadBegin,
            CKeyCode::Media(media_key_code) => KeyCode::Media(adapt(media_key_code)),
            CKeyCode::Modifier(modifier_key_code) => KeyCode::Modifier(adapt(modifier_key_code)),
        }
    }
}

impl AdaptFrom<platform::Event> for Event {
    fn adapt_from(event: platform::Event) -> Self {
        match event {
            platform::Event::Key(key) => Self::Key(adapt(key)),
            platform::Event::Mouse(mouse) => Self::Mouse(adapt(mouse)),
            #[cfg(not(windows))]
            platform::Event::WindowResized(termina::WindowSize { rows, cols, .. }) => {
                Self::Resize(cols, rows)
            }
            #[cfg(windows)]
            platform::Event::Resize(cols, rows) => Self::Resize(cols, rows),
            #[cfg(not(windows))]
            platform::Event::FocusIn => Self::FocusGained,
            #[cfg(not(windows))]
            platform::Event::FocusOut => Self::FocusLost,
            #[cfg(windows)]
            platform::Event::FocusGained => Self::FocusGained,
            #[cfg(windows)]
            platform::Event::FocusLost => Self::FocusLost,
            platform::Event::Paste(text) => Self::Paste(text),
            #[cfg(not(windows))]
            _ => unreachable!("escape reports are handled before portable input adaptation"),
        }
    }
}

impl AdaptFrom<platform::MouseEvent> for MouseEvent {
    fn adapt_from(
        platform::MouseEvent {
            kind,
            column,
            row,
            modifiers,
        }: platform::MouseEvent,
    ) -> Self {
        Self {
            kind: adapt(kind),
            column,
            row,
            modifiers: adapt(modifiers),
        }
    }
}

impl AdaptFrom<platform::MouseEventKind> for MouseEventKind {
    fn adapt_from(kind: platform::MouseEventKind) -> Self {
        match kind {
            platform::MouseEventKind::Down(button) => Self::Down(adapt(button)),
            platform::MouseEventKind::Up(button) => Self::Up(adapt(button)),
            platform::MouseEventKind::Drag(button) => Self::Drag(adapt(button)),
            platform::MouseEventKind::Moved => Self::Moved,
            platform::MouseEventKind::ScrollDown => Self::ScrollDown,
            platform::MouseEventKind::ScrollUp => Self::ScrollUp,
            platform::MouseEventKind::ScrollLeft => Self::ScrollLeft,
            platform::MouseEventKind::ScrollRight => Self::ScrollRight,
        }
    }
}

impl AdaptFrom<platform::MouseButton> for MouseButton {
    fn adapt_from(button: platform::MouseButton) -> Self {
        match button {
            platform::MouseButton::Left => MouseButton::Left,
            platform::MouseButton::Right => MouseButton::Right,
            platform::MouseButton::Middle => MouseButton::Middle,
        }
    }
}

impl AdaptFrom<platform::KeyEvent> for KeyEvent {
    fn adapt_from(
        platform::KeyEvent {
            code, modifiers, ..
        }: platform::KeyEvent,
    ) -> Self {
        if code == platform::KeyCode::BackTab {
            // special case for BackTab -> Shift-Tab
            let mut modifiers: KeyModifiers = adapt(modifiers);
            modifiers.insert(KeyModifiers::SHIFT);
            Self {
                code: KeyCode::Tab,
                modifiers,
            }
        } else {
            Self {
                code: adapt(code),
                modifiers: adapt(modifiers),
            }
        }
    }
}

impl AdaptFrom<KeyEvent> for platform::KeyEvent {
    fn adapt_from(KeyEvent { code, modifiers }: KeyEvent) -> Self {
        if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
            // special case for Shift-Tab -> BackTab
            let mut modifiers = modifiers;
            modifiers.remove(KeyModifiers::SHIFT);
            platform::KeyEvent {
                code: platform::KeyCode::BackTab,
                modifiers: adapt(modifiers),
                kind: platform::KeyEventKind::Press,
                state: platform::KeyEventState::NONE,
            }
        } else {
            platform::KeyEvent {
                code: adapt(code),
                modifiers: adapt(modifiers),
                kind: platform::KeyEventKind::Press,
                state: platform::KeyEventState::NONE,
            }
        }
    }
}

#[cfg(not(windows))]
impl AdaptFrom<UnderlineStyle> for termina::style::Underline {
    fn adapt_from(style: UnderlineStyle) -> Self {
        match style {
            UnderlineStyle::Reset => Self::None,
            UnderlineStyle::Line => Self::Single,
            UnderlineStyle::Curl => Self::Curly,
            UnderlineStyle::Dotted => Self::Dotted,
            UnderlineStyle::Dashed => Self::Dashed,
            UnderlineStyle::DoubleLine => Self::Double,
        }
    }
}

#[cfg(windows)]
impl AdaptFrom<UnderlineStyle> for crossterm::style::Attribute {
    fn adapt_from(style: UnderlineStyle) -> Self {
        match style {
            UnderlineStyle::Line => crossterm::style::Attribute::Underlined,
            UnderlineStyle::Curl => crossterm::style::Attribute::Undercurled,
            UnderlineStyle::Dotted => crossterm::style::Attribute::Underdotted,
            UnderlineStyle::Dashed => crossterm::style::Attribute::Underdashed,
            UnderlineStyle::DoubleLine => crossterm::style::Attribute::DoubleUnderlined,
            UnderlineStyle::Reset => crossterm::style::Attribute::NoUnderline,
        }
    }
}

#[cfg(not(windows))]
impl AdaptFrom<termina::escape::csi::ThemeMode> for ui_core::theme::Mode {
    fn adapt_from(mode: termina::escape::csi::ThemeMode) -> Self {
        match mode {
            termina::escape::csi::ThemeMode::Dark => Self::Dark,
            termina::escape::csi::ThemeMode::Light => Self::Light,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_round_trips_preserve_modifiers_and_normalize_back_tab() {
        let codes = [
            KeyCode::Tab,
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::F(12),
            KeyCode::Char('界'),
            KeyCode::Media(MediaKeyCode::PlayPause),
            KeyCode::Modifier(ModifierKeyCode::RightControl),
        ];
        for code in codes {
            for bits in 0..=KeyModifiers::all().bits() {
                let modifiers = KeyModifiers::from_bits(bits).unwrap();
                let key = KeyEvent { code, modifiers };
                let native: platform::KeyEvent = adapt(key);
                assert_eq!(adapt::<_, KeyEvent>(native), key);
            }
        }
        let mut native: platform::KeyEvent = adapt(KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
        });
        native.code = platform::KeyCode::BackTab;
        assert_eq!(
            adapt::<_, KeyEvent>(native),
            KeyEvent {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::SHIFT
            }
        );
    }

    #[test]
    fn mouse_and_paste_keep_portable_payloads() {
        let mouse = platform::MouseEvent {
            kind: platform::MouseEventKind::Drag(platform::MouseButton::Right),
            column: 17,
            row: 5,
            modifiers: adapt(KeyModifiers::CONTROL | KeyModifiers::ALT),
        };
        assert_eq!(
            adapt::<_, MouseEvent>(mouse),
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Right),
                column: 17,
                row: 5,
                modifiers: KeyModifiers::CONTROL | KeyModifiers::ALT,
            }
        );
        assert_eq!(
            adapt::<_, Event>(platform::Event::Paste("😀\r\n".into())),
            Event::Paste("😀\r\n".into())
        );
    }
}
