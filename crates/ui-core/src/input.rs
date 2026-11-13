//! Backend-neutral input events and key-notation parsing.
//!
//! Native backends adapt their events into these types before keymaps and
//! editor commands receive them.
use anyhow::{anyhow, Error};
use serde::de::{self, Deserialize, Deserializer};
use std::fmt;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub use crate::keyboard::{KeyCode, KeyModifiers, MediaKeyCode, ModifierKeyCode};

#[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Hash)]
pub enum Event {
    FocusGained,
    FocusLost,
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize(u16, u16),
    IdleTimeout,
}

#[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
pub struct MouseEvent {
    /// The kind of mouse event that was caused.
    pub kind: MouseEventKind,
    /// The column that the event occurred on.
    pub column: u16,
    /// The row that the event occurred on.
    pub row: u16,
    /// The key modifiers active when the event occurred.
    pub modifiers: KeyModifiers,
}

#[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
pub enum MouseEventKind {
    /// Pressed mouse button. Contains the button that was pressed.
    Down(MouseButton),
    /// Released mouse button. Contains the button that was released.
    Up(MouseButton),
    /// Moved the mouse cursor while pressing the contained mouse button.
    Drag(MouseButton),
    /// Moved the mouse cursor while not pressing a mouse button.
    Moved,
    /// Scrolled mouse wheel downwards (towards the user).
    ScrollDown,
    /// Scrolled mouse wheel upwards (away from the user).
    ScrollUp,
    /// Scrolled mouse wheel leftwards.
    ScrollLeft,
    /// Scrolled mouse wheel rightwards.
    ScrollRight,
}

/// Represents a mouse button.
#[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
pub enum MouseButton {
    /// Left mouse button.
    Left,
    /// Right mouse button.
    Right,
    /// Middle mouse button.
    Middle,
}
/// Represents a key event.
// We use a newtype here because we want to customize Deserialize and Display.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    // TODO: termina now supports kind & state if terminal supports kitty's extended protocol
}

impl KeyEvent {
    /// If a character was pressed, return it.
    pub fn char(&self) -> Option<char> {
        match self.code {
            KeyCode::Char(ch) => Some(ch),
            _ => None,
        }
    }

    /// Format the key in such a way that a concatenated sequence
    /// of keys can be read easily.
    ///
    /// ```
    /// # use std::str::FromStr;
    /// # use ui_core::input::KeyEvent;
    ///
    /// let k = KeyEvent::from_str("w").unwrap().key_sequence_format();
    /// assert_eq!(k, "w");
    ///
    /// let k = KeyEvent::from_str("C-w").unwrap().key_sequence_format();
    /// assert_eq!(k, "<C-w>");
    ///
    /// let k = KeyEvent::from_str(" ").unwrap().key_sequence_format();
    /// assert_eq!(k, "<space>");
    /// ```
    pub fn key_sequence_format(&self) -> String {
        let s = self.to_string();
        if s.graphemes(true).count() > 1 {
            format!("<{}>", s)
        } else {
            s
        }
    }
}

pub(crate) mod keys {
    pub(crate) const BACKSPACE: &str = "backspace";
    pub(crate) const ENTER: &str = "ret";
    pub(crate) const LEFT: &str = "left";
    pub(crate) const RIGHT: &str = "right";
    pub(crate) const UP: &str = "up";
    pub(crate) const DOWN: &str = "down";
    pub(crate) const HOME: &str = "home";
    pub(crate) const END: &str = "end";
    pub(crate) const PAGEUP: &str = "pageup";
    pub(crate) const PAGEDOWN: &str = "pagedown";
    pub(crate) const TAB: &str = "tab";
    pub(crate) const DELETE: &str = "del";
    pub(crate) const INSERT: &str = "ins";
    pub(crate) const NULL: &str = "null";
    pub(crate) const ESC: &str = "esc";
    pub(crate) const SPACE: &str = "space";
    pub(crate) const MINUS: &str = "minus";
    pub(crate) const LESS_THAN: &str = "lt";
    pub(crate) const GREATER_THAN: &str = "gt";
    pub(crate) const CAPS_LOCK: &str = "capslock";
    pub(crate) const SCROLL_LOCK: &str = "scrolllock";
    pub(crate) const NUM_LOCK: &str = "numlock";
    pub(crate) const PRINT_SCREEN: &str = "printscreen";
    pub(crate) const PAUSE: &str = "pause";
    pub(crate) const MENU: &str = "menu";
    pub(crate) const KEYPAD_BEGIN: &str = "keypadbegin";
    pub(crate) const PLAY: &str = "play";
    pub(crate) const PAUSE_MEDIA: &str = "pausemedia";
    pub(crate) const PLAY_PAUSE: &str = "playpause";
    pub(crate) const REVERSE: &str = "reverse";
    pub(crate) const STOP: &str = "stop";
    pub(crate) const FAST_FORWARD: &str = "fastforward";
    pub(crate) const REWIND: &str = "rewind";
    pub(crate) const TRACK_NEXT: &str = "tracknext";
    pub(crate) const TRACK_PREVIOUS: &str = "trackprevious";
    pub(crate) const RECORD: &str = "record";
    pub(crate) const LOWER_VOLUME: &str = "lowervolume";
    pub(crate) const RAISE_VOLUME: &str = "raisevolume";
    pub(crate) const MUTE_VOLUME: &str = "mutevolume";
    pub(crate) const LEFT_SHIFT: &str = "leftshift";
    pub(crate) const LEFT_CONTROL: &str = "leftcontrol";
    pub(crate) const LEFT_ALT: &str = "leftalt";
    pub(crate) const LEFT_SUPER: &str = "leftsuper";
    pub(crate) const LEFT_HYPER: &str = "lefthyper";
    pub(crate) const LEFT_META: &str = "leftmeta";
    pub(crate) const RIGHT_SHIFT: &str = "rightshift";
    pub(crate) const RIGHT_CONTROL: &str = "rightcontrol";
    pub(crate) const RIGHT_ALT: &str = "rightalt";
    pub(crate) const RIGHT_SUPER: &str = "rightsuper";
    pub(crate) const RIGHT_HYPER: &str = "righthyper";
    pub(crate) const RIGHT_META: &str = "rightmeta";
    pub(crate) const ISO_LEVEL_3_SHIFT: &str = "isolevel3shift";
    pub(crate) const ISO_LEVEL_5_SHIFT: &str = "isolevel5shift";
}

impl fmt::Display for KeyEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{}{}{}{}",
            if self.modifiers.contains(KeyModifiers::SUPER) {
                "Meta-"
            } else {
                ""
            },
            if self.modifiers.contains(KeyModifiers::SHIFT) {
                "S-"
            } else {
                ""
            },
            if self.modifiers.contains(KeyModifiers::ALT) {
                "A-"
            } else {
                ""
            },
            if self.modifiers.contains(KeyModifiers::CONTROL) {
                "C-"
            } else {
                ""
            },
        ))?;
        match self.key_label() {
            KeyLabel::Named(label) => f.write_str(label),
            KeyLabel::Function(number) => write!(f, "F{number}"),
            KeyLabel::Char(character) => write!(f, "{character}"),
        }
    }
}

// Named labels are ASCII; character labels retain their Unicode display width.
enum KeyLabel {
    Named(&'static str),
    Function(u8),
    Char(char),
}

impl KeyEvent {
    fn key_label(&self) -> KeyLabel {
        match self.code {
            KeyCode::Backspace => KeyLabel::Named(keys::BACKSPACE),
            KeyCode::Enter => KeyLabel::Named(keys::ENTER),
            KeyCode::Left => KeyLabel::Named(keys::LEFT),
            KeyCode::Right => KeyLabel::Named(keys::RIGHT),
            KeyCode::Up => KeyLabel::Named(keys::UP),
            KeyCode::Down => KeyLabel::Named(keys::DOWN),
            KeyCode::Home => KeyLabel::Named(keys::HOME),
            KeyCode::End => KeyLabel::Named(keys::END),
            KeyCode::PageUp => KeyLabel::Named(keys::PAGEUP),
            KeyCode::PageDown => KeyLabel::Named(keys::PAGEDOWN),
            KeyCode::Tab => KeyLabel::Named(keys::TAB),
            KeyCode::Delete => KeyLabel::Named(keys::DELETE),
            KeyCode::Insert => KeyLabel::Named(keys::INSERT),
            KeyCode::Null => KeyLabel::Named(keys::NULL),
            KeyCode::Esc => KeyLabel::Named(keys::ESC),
            KeyCode::Char(' ') => KeyLabel::Named(keys::SPACE),
            KeyCode::Char('-') => KeyLabel::Named(keys::MINUS),
            KeyCode::Char('<') => KeyLabel::Named(keys::LESS_THAN),
            KeyCode::Char('>') => KeyLabel::Named(keys::GREATER_THAN),
            KeyCode::F(i) => KeyLabel::Function(i),
            KeyCode::Char(c) => KeyLabel::Char(c),
            KeyCode::CapsLock => KeyLabel::Named(keys::CAPS_LOCK),
            KeyCode::ScrollLock => KeyLabel::Named(keys::SCROLL_LOCK),
            KeyCode::NumLock => KeyLabel::Named(keys::NUM_LOCK),
            KeyCode::PrintScreen => KeyLabel::Named(keys::PRINT_SCREEN),
            KeyCode::Pause => KeyLabel::Named(keys::PAUSE),
            KeyCode::Menu => KeyLabel::Named(keys::MENU),
            KeyCode::KeypadBegin => KeyLabel::Named(keys::KEYPAD_BEGIN),
            KeyCode::Media(MediaKeyCode::Play) => KeyLabel::Named(keys::PLAY),
            KeyCode::Media(MediaKeyCode::Pause) => KeyLabel::Named(keys::PAUSE_MEDIA),
            KeyCode::Media(MediaKeyCode::PlayPause) => KeyLabel::Named(keys::PLAY_PAUSE),
            KeyCode::Media(MediaKeyCode::Stop) => KeyLabel::Named(keys::STOP),
            KeyCode::Media(MediaKeyCode::Reverse) => KeyLabel::Named(keys::REVERSE),
            KeyCode::Media(MediaKeyCode::FastForward) => KeyLabel::Named(keys::FAST_FORWARD),
            KeyCode::Media(MediaKeyCode::Rewind) => KeyLabel::Named(keys::REWIND),
            KeyCode::Media(MediaKeyCode::TrackNext) => KeyLabel::Named(keys::TRACK_NEXT),
            KeyCode::Media(MediaKeyCode::TrackPrevious) => KeyLabel::Named(keys::TRACK_PREVIOUS),
            KeyCode::Media(MediaKeyCode::Record) => KeyLabel::Named(keys::RECORD),
            KeyCode::Media(MediaKeyCode::LowerVolume) => KeyLabel::Named(keys::LOWER_VOLUME),
            KeyCode::Media(MediaKeyCode::RaiseVolume) => KeyLabel::Named(keys::RAISE_VOLUME),
            KeyCode::Media(MediaKeyCode::MuteVolume) => KeyLabel::Named(keys::MUTE_VOLUME),
            KeyCode::Modifier(ModifierKeyCode::LeftShift) => KeyLabel::Named(keys::LEFT_SHIFT),
            KeyCode::Modifier(ModifierKeyCode::LeftControl) => KeyLabel::Named(keys::LEFT_CONTROL),
            KeyCode::Modifier(ModifierKeyCode::LeftAlt) => KeyLabel::Named(keys::LEFT_ALT),
            KeyCode::Modifier(ModifierKeyCode::LeftSuper) => KeyLabel::Named(keys::LEFT_SUPER),
            KeyCode::Modifier(ModifierKeyCode::LeftHyper) => KeyLabel::Named(keys::LEFT_HYPER),
            KeyCode::Modifier(ModifierKeyCode::LeftMeta) => KeyLabel::Named(keys::LEFT_META),
            KeyCode::Modifier(ModifierKeyCode::RightShift) => KeyLabel::Named(keys::RIGHT_SHIFT),
            KeyCode::Modifier(ModifierKeyCode::RightControl) => {
                KeyLabel::Named(keys::RIGHT_CONTROL)
            }
            KeyCode::Modifier(ModifierKeyCode::RightAlt) => KeyLabel::Named(keys::RIGHT_ALT),
            KeyCode::Modifier(ModifierKeyCode::RightSuper) => KeyLabel::Named(keys::RIGHT_SUPER),
            KeyCode::Modifier(ModifierKeyCode::RightHyper) => KeyLabel::Named(keys::RIGHT_HYPER),
            KeyCode::Modifier(ModifierKeyCode::RightMeta) => KeyLabel::Named(keys::RIGHT_META),
            KeyCode::Modifier(ModifierKeyCode::IsoLevel3Shift) => {
                KeyLabel::Named(keys::ISO_LEVEL_3_SHIFT)
            }
            KeyCode::Modifier(ModifierKeyCode::IsoLevel5Shift) => {
                KeyLabel::Named(keys::ISO_LEVEL_5_SHIFT)
            }
        }
    }
}

impl UnicodeWidthStr for KeyEvent {
    fn width(&self) -> usize {
        use unicode_width::UnicodeWidthChar;
        let mut width = match self.key_label() {
            KeyLabel::Named(label) => label.len(),
            KeyLabel::Function(0..=9) => 2,
            KeyLabel::Function(10..=99) => 3,
            KeyLabel::Function(_) => 4,
            KeyLabel::Char(character) => character.width().unwrap_or(0),
        };
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            width += 2;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            width += 2;
        }
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            width += 2;
        }
        if self.modifiers.contains(KeyModifiers::SUPER) {
            // "-Meta"
            width += 5;
        }
        width
    }

    fn width_cjk(&self) -> usize {
        self.width()
    }
}

impl std::str::FromStr for KeyEvent {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut tokens: Vec<_> = s.split('-').collect();
        let mut code = match tokens.pop().ok_or_else(|| anyhow!("Missing key code"))? {
            keys::BACKSPACE => KeyCode::Backspace,
            keys::ENTER => KeyCode::Enter,
            keys::LEFT => KeyCode::Left,
            keys::RIGHT => KeyCode::Right,
            keys::UP => KeyCode::Up,
            keys::DOWN => KeyCode::Down,
            keys::HOME => KeyCode::Home,
            keys::END => KeyCode::End,
            keys::PAGEUP => KeyCode::PageUp,
            keys::PAGEDOWN => KeyCode::PageDown,
            keys::TAB => KeyCode::Tab,
            keys::DELETE => KeyCode::Delete,
            keys::INSERT => KeyCode::Insert,
            keys::NULL => KeyCode::Null,
            keys::ESC => KeyCode::Esc,
            keys::SPACE => KeyCode::Char(' '),
            keys::MINUS => KeyCode::Char('-'),
            keys::LESS_THAN => KeyCode::Char('<'),
            keys::GREATER_THAN => KeyCode::Char('>'),
            keys::CAPS_LOCK => KeyCode::CapsLock,
            keys::SCROLL_LOCK => KeyCode::ScrollLock,
            keys::NUM_LOCK => KeyCode::NumLock,
            keys::PRINT_SCREEN => KeyCode::PrintScreen,
            keys::PAUSE => KeyCode::Pause,
            keys::MENU => KeyCode::Menu,
            keys::KEYPAD_BEGIN => KeyCode::KeypadBegin,
            keys::PLAY => KeyCode::Media(MediaKeyCode::Play),
            keys::PAUSE_MEDIA => KeyCode::Media(MediaKeyCode::Pause),
            keys::PLAY_PAUSE => KeyCode::Media(MediaKeyCode::PlayPause),
            keys::STOP => KeyCode::Media(MediaKeyCode::Stop),
            keys::REVERSE => KeyCode::Media(MediaKeyCode::Reverse),
            keys::FAST_FORWARD => KeyCode::Media(MediaKeyCode::FastForward),
            keys::REWIND => KeyCode::Media(MediaKeyCode::Rewind),
            keys::TRACK_NEXT => KeyCode::Media(MediaKeyCode::TrackNext),
            keys::TRACK_PREVIOUS => KeyCode::Media(MediaKeyCode::TrackPrevious),
            keys::RECORD => KeyCode::Media(MediaKeyCode::Record),
            keys::LOWER_VOLUME => KeyCode::Media(MediaKeyCode::LowerVolume),
            keys::RAISE_VOLUME => KeyCode::Media(MediaKeyCode::RaiseVolume),
            keys::MUTE_VOLUME => KeyCode::Media(MediaKeyCode::MuteVolume),
            keys::LEFT_SHIFT => KeyCode::Modifier(ModifierKeyCode::LeftShift),
            keys::LEFT_CONTROL => KeyCode::Modifier(ModifierKeyCode::LeftControl),
            keys::LEFT_ALT => KeyCode::Modifier(ModifierKeyCode::LeftAlt),
            keys::LEFT_SUPER => KeyCode::Modifier(ModifierKeyCode::LeftSuper),
            keys::LEFT_HYPER => KeyCode::Modifier(ModifierKeyCode::LeftHyper),
            keys::LEFT_META => KeyCode::Modifier(ModifierKeyCode::LeftMeta),
            keys::RIGHT_SHIFT => KeyCode::Modifier(ModifierKeyCode::RightShift),
            keys::RIGHT_CONTROL => KeyCode::Modifier(ModifierKeyCode::RightControl),
            keys::RIGHT_ALT => KeyCode::Modifier(ModifierKeyCode::RightAlt),
            keys::RIGHT_SUPER => KeyCode::Modifier(ModifierKeyCode::RightSuper),
            keys::RIGHT_HYPER => KeyCode::Modifier(ModifierKeyCode::RightHyper),
            keys::RIGHT_META => KeyCode::Modifier(ModifierKeyCode::RightMeta),
            keys::ISO_LEVEL_3_SHIFT => KeyCode::Modifier(ModifierKeyCode::IsoLevel3Shift),
            keys::ISO_LEVEL_5_SHIFT => KeyCode::Modifier(ModifierKeyCode::IsoLevel5Shift),
            single if single.chars().count() == 1 => KeyCode::Char(single.chars().next().unwrap()),
            function if function.len() > 1 && function.starts_with('F') => {
                let function: String = function.chars().skip(1).collect();
                let function = str::parse::<u8>(&function)?;
                (function > 0 && function < 25)
                    .then_some(KeyCode::F(function))
                    .ok_or_else(|| anyhow!("Invalid function key '{}'", function))?
            }
            // Checking that the last token is empty ensures that this branch is only taken if
            // `-` is used as a code. For example this branch will not be taken for `S-` (which is
            // missing a code).
            _ if s.ends_with('-') && tokens.last().is_some_and(|t| t.is_empty()) => {
                if s == "-" {
                    return Ok(KeyEvent {
                        code: KeyCode::Char('-'),
                        modifiers: KeyModifiers::empty(),
                    });
                } else {
                    let suggestion = format!("{}-{}", s.trim_end_matches('-'), keys::MINUS);
                    return Err(anyhow!(
                        "Key '-' cannot be used with modifiers, use '{}' instead",
                        suggestion
                    ));
                }
            }
            invalid => return Err(anyhow!("Invalid key code '{}'", invalid)),
        };

        let mut modifiers = KeyModifiers::empty();
        for token in tokens {
            let flag = match token {
                "S" => KeyModifiers::SHIFT,
                "A" => KeyModifiers::ALT,
                "C" => KeyModifiers::CONTROL,
                "Meta" | "Cmd" | "Win" => KeyModifiers::SUPER,
                _ => return Err(anyhow!("Invalid key modifier '{}-'", token)),
            };

            if modifiers.contains(flag) {
                return Err(anyhow!("Repeated key modifier '{}-'", token));
            }
            modifiers.insert(flag);
        }

        // Normalize character keys so that characters like C-S-r and C-R
        // are represented by equal KeyEvents.
        match code {
            KeyCode::Char(ch)
                if ch.is_ascii_lowercase() && modifiers.contains(KeyModifiers::SHIFT) =>
            {
                code = KeyCode::Char(ch.to_ascii_uppercase());
                modifiers.remove(KeyModifiers::SHIFT);
            }
            _ => (),
        }

        Ok(KeyEvent { code, modifiers })
    }
}

impl<'de> Deserialize<'de> for KeyEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

pub fn parse_macro(keys_str: &str) -> anyhow::Result<Vec<KeyEvent>> {
    use anyhow::Context;
    let mut keys_res: anyhow::Result<_> = Ok(Vec::new());
    let mut i = 0;
    while let Ok(keys) = &mut keys_res {
        if i >= keys_str.len() {
            break;
        }
        if !keys_str.is_char_boundary(i) {
            i += 1;
            continue;
        }

        let s = &keys_str[i..];
        let mut end_i = 1;
        while !s.is_char_boundary(end_i) {
            end_i += 1;
        }
        let c = &s[..end_i];
        if c == ">" {
            keys_res = Err(anyhow!("Unmatched '>'"));
        } else if c != "<" {
            keys.push(if c == "-" { keys::MINUS } else { c });
            i += end_i;
        } else {
            match s.find('>').context("'>' expected") {
                Ok(end_i) => {
                    keys.push(&s[1..end_i]);
                    i += end_i + 1;
                }
                Err(err) => keys_res = Err(err),
            }
        }
    }
    keys_res.and_then(|keys| keys.into_iter().map(str::parse).collect())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn displayed_key_widths_match() {
        for label in [
            "lt",
            "gt",
            "space",
            "minus",
            "backspace",
            "ret",
            "pageup",
            "capslock",
            "playpause",
            "trackprevious",
            "leftshift",
            "isolevel5shift",
            "F1",
            "F9",
            "F10",
            "F24",
            "a",
            "é",
            "界",
            "\u{301}",
        ] {
            for prefix in ["", "S-", "A-", "C-", "Meta-", "Meta-S-A-C-"] {
                let key: KeyEvent = format!("{prefix}{label}").parse().unwrap();
                let displayed = key.to_string();
                assert_eq!(key.width(), displayed.width(), "{displayed}");
                assert_eq!(key.width_cjk(), key.width(), "{displayed}");
            }
        }

        let key: KeyEvent = "C-A-S-Meta-lt".parse().unwrap();
        assert_eq!(key.to_string(), "Meta-S-A-C-lt");

        // Backends can construct function keys outside the parser's F1-F24 range.
        for number in [0, 99, 100, 255] {
            let key = KeyEvent {
                code: KeyCode::F(number),
                modifiers: KeyModifiers::NONE,
            };
            assert_eq!(key.width(), key.to_string().width());
        }
    }

    #[test]
    fn parsing_unmodified_keys() {
        assert_eq!(
            str::parse::<KeyEvent>("backspace").unwrap(),
            KeyEvent {
                code: KeyCode::Backspace,
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("left").unwrap(),
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>(",").unwrap(),
            KeyEvent {
                code: KeyCode::Char(','),
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("w").unwrap(),
            KeyEvent {
                code: KeyCode::Char('w'),
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("F12").unwrap(),
            KeyEvent {
                code: KeyCode::F(12),
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("%").unwrap(),
            KeyEvent {
                code: KeyCode::Char('%'),
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>(";").unwrap(),
            KeyEvent {
                code: KeyCode::Char(';'),
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>(">").unwrap(),
            KeyEvent {
                code: KeyCode::Char('>'),
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("<").unwrap(),
            KeyEvent {
                code: KeyCode::Char('<'),
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("+").unwrap(),
            KeyEvent {
                code: KeyCode::Char('+'),
                modifiers: KeyModifiers::NONE
            }
        );
        assert_eq!(
            str::parse::<KeyEvent>("-").unwrap(),
            KeyEvent {
                code: KeyCode::Char('-'),
                modifiers: KeyModifiers::NONE,
            }
        );
    }

    #[test]
    fn parsing_modified_keys() {
        assert_eq!(
            str::parse::<KeyEvent>("S-minus").unwrap(),
            KeyEvent {
                code: KeyCode::Char('-'),
                modifiers: KeyModifiers::SHIFT
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("C-A-S-F12").unwrap(),
            KeyEvent {
                code: KeyCode::F(12),
                modifiers: KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("S-C-2").unwrap(),
            KeyEvent {
                code: KeyCode::Char('2'),
                modifiers: KeyModifiers::SHIFT | KeyModifiers::CONTROL
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("A-C-+").unwrap(),
            KeyEvent {
                code: KeyCode::Char('+'),
                modifiers: KeyModifiers::ALT | KeyModifiers::CONTROL
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("C-S-r").unwrap(),
            str::parse::<KeyEvent>("C-R").unwrap(),
        );

        assert_eq!(
            str::parse::<KeyEvent>("S-w").unwrap(),
            KeyEvent {
                code: KeyCode::Char('W'),
                modifiers: KeyModifiers::NONE
            }
        );

        assert_eq!(
            str::parse::<KeyEvent>("Meta-c").unwrap(),
            KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::SUPER
            }
        );
        assert_eq!(
            str::parse::<KeyEvent>("Win-s").unwrap(),
            KeyEvent {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::SUPER
            }
        );
        assert_eq!(
            str::parse::<KeyEvent>("Cmd-d").unwrap(),
            KeyEvent {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::SUPER
            }
        );
    }

    #[test]
    fn parsing_nonsensical_keys_fails() {
        assert!(str::parse::<KeyEvent>("F25").is_err());
        assert!(str::parse::<KeyEvent>("F0").is_err());
        assert!(str::parse::<KeyEvent>("aaa").is_err());
        assert!(str::parse::<KeyEvent>("S-S-a").is_err());
        assert!(str::parse::<KeyEvent>("C-A-S-C-1").is_err());
        assert!(str::parse::<KeyEvent>("FU").is_err());
        assert!(str::parse::<KeyEvent>("123").is_err());
        assert!(str::parse::<KeyEvent>("S--").is_err());
        assert!(str::parse::<KeyEvent>("S-").is_err());
        assert!(str::parse::<KeyEvent>("S-percent").is_err());
    }

    #[test]
    fn parsing_unsupported_named_keys() {
        assert!(str::parse::<KeyEvent>("plus").is_err());
        assert!(str::parse::<KeyEvent>("percent").is_err());
        assert!(str::parse::<KeyEvent>("semicolon").is_err());
    }

    #[test]
    fn parsing_valid_macros() {
        assert_eq!(
            parse_macro("xdo").ok(),
            Some(vec![
                KeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('d'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('o'),
                    modifiers: KeyModifiers::NONE,
                },
            ]),
        );

        assert_eq!(
            parse_macro("<C-w>v<C-w>h<C-o>xx<A-s>").ok(),
            Some(vec![
                KeyEvent {
                    code: KeyCode::Char('w'),
                    modifiers: KeyModifiers::CONTROL,
                },
                KeyEvent {
                    code: KeyCode::Char('v'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('w'),
                    modifiers: KeyModifiers::CONTROL,
                },
                KeyEvent {
                    code: KeyCode::Char('h'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('o'),
                    modifiers: KeyModifiers::CONTROL,
                },
                KeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('s'),
                    modifiers: KeyModifiers::ALT,
                },
            ])
        );

        assert_eq!(
            parse_macro(":o foo.bar<ret>").ok(),
            Some(vec![
                KeyEvent {
                    code: KeyCode::Char(':'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('o'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char(' '),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('f'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('o'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('o'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('.'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('b'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('a'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('r'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::NONE,
                },
            ])
        );

        assert_eq!(
            parse_macro(":w aa-bb.txt<ret>").ok(),
            Some(vec![
                KeyEvent {
                    code: KeyCode::Char(':'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('w'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char(' '),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('a'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('a'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('-'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('b'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('b'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('.'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('t'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Char('t'),
                    modifiers: KeyModifiers::NONE,
                },
                KeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::NONE,
                },
            ])
        );
    }

    #[test]
    fn parsing_invalid_macros_fails() {
        assert!(parse_macro("abc<C-").is_err());
        assert!(parse_macro("abc>123").is_err());
        assert!(parse_macro("wd<foo>").is_err());
    }
}
