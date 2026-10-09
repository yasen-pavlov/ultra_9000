//! Physical key routing: Key::{from_evdev,from_mac_keycode}, Keymap and parse_chord.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Palette,
    PaletteProject,
    NewAgent,
    NewTerminal,
    NewNeovim,
    Close,
    JumpWaiting,
    Item(u8),
    ItemPrevious,
    ItemNext,
    ProjectPrevious,
    ProjectNext,
    FocusSidebar,
    ToggleSidebar,
    Settings,
    Shortcuts,
    Hide,
    Quit,
}
impl Action {
    pub fn name(self) -> String {
        match self {
            Self::Item(n) => format!("item-{n}"),
            a => match a {
                Self::Palette => "palette",
                Self::PaletteProject => "palette-project",
                Self::NewAgent => "new-agent",
                Self::NewTerminal => "new-terminal",
                Self::NewNeovim => "new-neovim",
                Self::Close => "close",
                Self::JumpWaiting => "jump-waiting",
                Self::ItemPrevious => "item-previous",
                Self::ItemNext => "item-next",
                Self::ProjectPrevious => "project-previous",
                Self::ProjectNext => "project-next",
                Self::FocusSidebar => "focus-sidebar",
                Self::ToggleSidebar => "toggle-sidebar",
                Self::Settings => "settings",
                Self::Shortcuts => "shortcuts",
                Self::Hide => "hide",
                Self::Quit => "quit",
                Self::Item(_) => unreachable!(),
            }
            .into(),
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::all().into_iter().find(|a| a.name() == s)
    }
    pub fn all() -> Vec<Self> {
        let mut v = vec![
            Self::Palette,
            Self::PaletteProject,
            Self::NewAgent,
            Self::NewTerminal,
            Self::NewNeovim,
            Self::Close,
            Self::JumpWaiting,
        ];
        v.extend((1..=9).map(Self::Item));
        v.extend([
            Self::ItemPrevious,
            Self::ItemNext,
            Self::ProjectPrevious,
            Self::ProjectNext,
            Self::FocusSidebar,
            Self::ToggleSidebar,
            Self::Settings,
            Self::Shortcuts,
            Self::Hide,
            Self::Quit,
        ]);
        v
    }
}
impl Serialize for Action {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.name())
    }
}
impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom("unknown shortcut action"))
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Comma,
    Period,
    Slash,
    Semicolon,
    Quote,
    Backquote,
    Backslash,
    BracketLeft,
    BracketRight,
    Minus,
    Equal,
    Space,
    Tab,
    Enter,
    Escape,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Print,
}
// (key, evdev, kVK); physical ANSI positions, never layout-produced characters.
const CODES: &[(Key, u16, u16, &str)] = &[
    (Key::A, 30, 0, "A"),
    (Key::B, 48, 11, "B"),
    (Key::C, 46, 8, "C"),
    (Key::D, 32, 2, "D"),
    (Key::E, 18, 14, "E"),
    (Key::F, 33, 3, "F"),
    (Key::G, 34, 5, "G"),
    (Key::H, 35, 4, "H"),
    (Key::I, 23, 34, "I"),
    (Key::J, 36, 38, "J"),
    (Key::K, 37, 40, "K"),
    (Key::L, 38, 37, "L"),
    (Key::M, 50, 46, "M"),
    (Key::N, 49, 45, "N"),
    (Key::O, 24, 31, "O"),
    (Key::P, 25, 35, "P"),
    (Key::Q, 16, 12, "Q"),
    (Key::R, 19, 15, "R"),
    (Key::S, 31, 1, "S"),
    (Key::T, 20, 17, "T"),
    (Key::U, 22, 32, "U"),
    (Key::V, 47, 9, "V"),
    (Key::W, 17, 13, "W"),
    (Key::X, 45, 7, "X"),
    (Key::Y, 21, 16, "Y"),
    (Key::Z, 44, 6, "Z"),
    (Key::Digit0, 11, 29, "0"),
    (Key::Digit1, 2, 18, "1"),
    (Key::Digit2, 3, 19, "2"),
    (Key::Digit3, 4, 20, "3"),
    (Key::Digit4, 5, 21, "4"),
    (Key::Digit5, 6, 23, "5"),
    (Key::Digit6, 7, 22, "6"),
    (Key::Digit7, 8, 26, "7"),
    (Key::Digit8, 9, 28, "8"),
    (Key::Digit9, 10, 25, "9"),
    (Key::Comma, 51, 43, ","),
    (Key::Period, 52, 47, "."),
    (Key::Slash, 53, 44, "/"),
    (Key::Semicolon, 39, 41, ";"),
    (Key::Quote, 40, 39, "'"),
    (Key::Backquote, 41, 50, "`"),
    (Key::Backslash, 43, 42, "\\"),
    (Key::BracketLeft, 26, 33, "["),
    (Key::BracketRight, 27, 30, "]"),
    (Key::Minus, 12, 27, "-"),
    (Key::Equal, 13, 24, "="),
    (Key::Space, 57, 49, "Space"),
    (Key::Tab, 15, 48, "Tab"),
    (Key::Enter, 28, 36, "Enter"),
    (Key::Escape, 1, 53, "Escape"),
    (Key::Up, 103, 126, "Up"),
    (Key::Down, 108, 125, "Down"),
    (Key::Left, 105, 123, "Left"),
    (Key::Right, 106, 124, "Right"),
    (Key::PageUp, 104, 116, "PageUp"),
    (Key::PageDown, 109, 121, "PageDown"),
    (Key::Home, 102, 115, "Home"),
    (Key::End, 107, 119, "End"),
    (Key::Print, 99, u16::MAX, "Print"),
];
impl Key {
    pub fn from_evdev(code: u16) -> Option<Self> {
        CODES.iter().find(|c| c.1 == code).map(|c| c.0)
    }
    pub fn from_mac_keycode(code: u16) -> Option<Self> {
        CODES
            .iter()
            .find(|c| c.2 != u16::MAX && c.2 == code)
            .map(|c| c.0)
    }
    fn name(self) -> &'static str {
        CODES.iter().find(|c| c.0 == self).unwrap().3
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub mods: Mods,
    pub key: Key,
}
pub fn parse_chord(s: &str) -> Result<Chord, String> {
    let mut mods = Mods::default();
    let parts: Vec<_> = s.split('+').collect();
    let (key, modifiers) = parts.split_last().ok_or("empty chord")?;
    for m in modifiers {
        let bit = match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => &mut mods.ctrl,
            "shift" => &mut mods.shift,
            "alt" | "option" => &mut mods.alt,
            "cmd" | "super" | "logo" | "meta" => &mut mods.logo,
            _ => return Err(format!("unknown modifier: {m}")),
        };
        if *bit {
            return Err(format!("duplicate modifier: {m}"));
        }
        *bit = true;
    }
    let key = CODES
        .iter()
        .find(|c| c.3.eq_ignore_ascii_case(key) || format!("{:?}", c.0).eq_ignore_ascii_case(key))
        .map(|c| c.0)
        .ok_or_else(|| format!("unknown key: {key}"))?;
    Ok(Chord { mods, key })
}
pub fn never_captured(platform: Platform, chord: &Chord) -> bool {
    let m = chord.mods;
    let k = chord.key;
    match platform {
        Platform::Linux => {
            let alias =
                m.ctrl && !m.shift && !m.alt && !m.logo && matches!(k, Key::PageUp | Key::PageDown);
            m.logo
                || !m.ctrl
                || (!m.shift && !alias)
                || matches!(
                    k,
                    Key::C
                        | Key::V
                        | Key::F
                        | Key::B
                        | Key::BracketLeft
                        | Key::O
                        | Key::P
                        | Key::I
                        | Key::K
                        | Key::Print
                        | Key::Equal
                        | Key::Minus
                        | Key::Escape
                )
        }
        Platform::MacOs => {
            !m.logo
                || matches!(
                    k,
                    Key::C | Key::V | Key::F | Key::B | Key::K | Key::Escape | Key::Print
                )
        }
    }
}
/// One `[keys.bindings]` entry: an empty chord unbinds the action.
fn parse_request(
    platform: Platform,
    name: &str,
    value: &toml::Value,
) -> Result<(Action, Option<Chord>), String> {
    let action = Action::parse(name).ok_or_else(|| format!("unknown action: {name}"))?;
    let text = value
        .as_str()
        .ok_or_else(|| format!("{name}: expected chord string"))?;
    if text.is_empty() {
        return Ok((action, None));
    }
    match parse_chord(text) {
        Ok(chord) if !never_captured(platform, &chord) => Ok((action, Some(chord))),
        Ok(_) => Err(format!("{name}: chord is never captured")),
        Err(e) => Err(format!("{name}: {e}")),
    }
}
#[derive(Clone, Debug)]
pub struct Keymap {
    platform: Platform,
    bindings: Vec<(Chord, Action)>,
}
impl Keymap {
    pub fn defaults(platform: Platform) -> Self {
        let mut map = Self {
            platform,
            bindings: Vec::new(),
        };
        let pairs = [
            (Action::Palette, "Space", "O"),
            (Action::PaletteProject, "R", "R"),
            (Action::NewAgent, "N", "N"),
            (Action::NewTerminal, "T", "T"),
            (Action::NewNeovim, "E", "E"),
            (Action::Close, "W", "W"),
            (Action::JumpWaiting, "J", "J"),
            (Action::FocusSidebar, "L", "L"),
            (Action::Settings, ",", ","),
            (Action::Shortcuts, "/", "/"),
            (Action::Hide, "H", "H"),
            (Action::Quit, "Q", "Q"),
        ];
        for (a, l, m) in pairs {
            let chord = format!(
                "{}{}",
                if platform == Platform::Linux {
                    "Ctrl+Shift+"
                } else {
                    "Cmd+"
                },
                if platform == Platform::Linux { l } else { m }
            );
            map.bindings.push((parse_chord(&chord).unwrap(), a));
        }
        for n in 1..=9 {
            let chord = format!(
                "{}{n}",
                if platform == Platform::Linux {
                    "Ctrl+Shift+"
                } else {
                    "Cmd+"
                }
            );
            map.bindings
                .push((parse_chord(&chord).unwrap(), Action::Item(n)));
        }
        let extras = if platform == Platform::Linux {
            vec![
                (Action::ItemPrevious, "Ctrl+Shift+Up"),
                (Action::ItemNext, "Ctrl+Shift+Down"),
                (Action::ItemPrevious, "Ctrl+PageUp"),
                (Action::ItemNext, "Ctrl+PageDown"),
                (Action::ProjectPrevious, "Ctrl+Shift+PageUp"),
                (Action::ProjectNext, "Ctrl+Shift+PageDown"),
                (Action::ToggleSidebar, "Ctrl+Shift+S"),
            ]
        } else {
            vec![
                (Action::ItemPrevious, "Cmd+Shift+["),
                (Action::ItemNext, "Cmd+Shift+]"),
                (Action::ProjectPrevious, "Cmd+Option+["),
                (Action::ProjectNext, "Cmd+Option+]"),
                (Action::ToggleSidebar, "Ctrl+Cmd+S"),
            ]
        };
        for (a, c) in extras {
            map.bindings.push((parse_chord(c).unwrap(), a));
        }
        map
    }
    pub fn with_overrides(platform: Platform, keys: Option<&toml::Table>) -> (Self, Vec<String>) {
        let mut map = Self::defaults(platform);
        let mut errors = Vec::new();
        if let Some(keys) = keys {
            for k in keys.keys().filter(|k| k.as_str() != "bindings") {
                errors.push(format!("unknown keys table: {k}"));
            }
            match keys.get("bindings").map(toml::Value::as_table) {
                Some(Some(bindings)) => map.apply_bindings(bindings, &mut errors),
                Some(None) => errors.push("bindings: expected table".into()),
                None => {}
            }
        }
        (map, errors)
    }
    fn apply_bindings(&mut self, bindings: &toml::Table, errors: &mut Vec<String>) {
        let mut requested = Vec::new();
        for (name, value) in bindings {
            match parse_request(self.platform, name, value) {
                Ok(request) => requested.push(request),
                Err(e) => errors.push(e),
            }
        }
        // Remove overridden actions as a batch, so moves/swaps do not depend on
        // TOML key ordering. Rejecting an override restores its defaults; repeat
        // until those restored defaults no longer collide with accepted requests.
        loop {
            let rejected = self.collisions(&requested);
            if rejected.is_empty() {
                break;
            }
            for action in &rejected {
                errors.push(format!("{}: chord already bound", action.name()));
            }
            requested.retain(|(action, _)| !rejected.contains(action));
        }
        self.bindings
            .retain(|(_, action)| !requested.iter().any(|(a, _)| a == action));
        self.bindings
            .extend(requested.into_iter().filter_map(|(a, c)| c.map(|c| (c, a))));
    }
    /// Requested actions whose chord collides with a kept default or another request.
    fn collisions(&self, requested: &[(Action, Option<Chord>)]) -> Vec<Action> {
        requested
            .iter()
            .filter_map(|(action, chord)| {
                let chord = chord.as_ref()?;
                let default_collision = self.bindings.iter().any(|(c, a)| {
                    c == chord
                        && a != action
                        && !requested.iter().any(|(overridden, _)| overridden == a)
                });
                let request_collision = requested
                    .iter()
                    .any(|(a, c)| a != action && c.as_ref() == Some(chord));
                (default_collision || request_collision).then_some(*action)
            })
            .collect()
    }
    pub fn lookup(&self, mods: Mods, key: Key) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(c, _)| c.mods == mods && c.key == key)
            .map(|(_, a)| *a)
    }
    pub fn label(&self, action: Action) -> Option<String> {
        self.bindings
            .iter()
            .find(|(_, a)| *a == action)
            .map(|(c, _)| {
                let mut s = String::new();
                if c.mods.ctrl {
                    s.push_str("Ctrl+");
                }
                if c.mods.logo && self.platform == Platform::MacOs {
                    s.push_str("Cmd+");
                }
                if c.mods.shift {
                    s.push_str("Shift+");
                }
                if c.mods.alt {
                    s.push_str(if self.platform == Platform::MacOs {
                        "Option+"
                    } else {
                        "Alt+"
                    });
                }
                if c.mods.logo && self.platform == Platform::Linux {
                    s.push_str("Super+");
                }
                s.push_str(c.key.name());
                s
            })
    }
    pub fn all(&self) -> Vec<(Action, Option<String>)> {
        Action::all()
            .into_iter()
            .map(|a| (a, self.label(a)))
            .collect()
    }
}
#[cfg(test)]
mod tests;
