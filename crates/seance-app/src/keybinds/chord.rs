//! Chord grammar: a modifier set plus a single key token, parsed from a config
//! `key` string and reconstructed from a live winit event for lookup.
//!
//! Matching is against the *logical* key (winit `Key`), preserving the
//! historical behavior and the user's "the plus key" mental model. The cost is
//! layout dependence (a binding follows the key that *produces* the character);
//! a physical-key option can layer on later without changing the grammar.

use winit::event::{KeyEvent, Modifiers};
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModSet(u8);

impl ModSet {
    pub const SHIFT: Self = Self(1 << 0);
    pub const CTRL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const SUPER: Self = Self(1 << 3);

    pub const fn empty() -> Self {
        Self(0)
    }

    fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl std::ops::BitOr for ModSet {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedToken {
    Enter,
    Escape,
    Tab,
    Space,
    Backspace,
    Delete,
    Minus,
    Equal,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyToken {
    Char(char),
    Named(NamedToken),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub mods: ModSet,
    pub key: KeyToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordParseError {
    Empty,
    NoKey,
    MultipleKeys,
    UnknownToken(String),
}

impl Chord {
    pub fn new(mods: ModSet, key: KeyToken) -> Self {
        Self { mods, key }
    }

    pub fn parse(spec: &str) -> Result<Chord, ChordParseError> {
        if spec.trim().is_empty() {
            return Err(ChordParseError::Empty);
        }
        let mut mods = ModSet::empty();
        let mut key: Option<KeyToken> = None;
        // `+` is always the separator; the literal key is spelled `plus`. An
        // empty segment (e.g. a trailing `+`) is skipped, so `ctrl+` is NoKey.
        for raw in spec.split('+') {
            let tok = raw.trim().to_ascii_lowercase();
            if tok.is_empty() {
                continue;
            }
            if let Some(bit) = modifier_bit(&tok) {
                mods.insert(bit);
                continue;
            }
            let parsed =
                parse_key_token(&tok).ok_or_else(|| ChordParseError::UnknownToken(tok.clone()))?;
            if key.is_some() {
                return Err(ChordParseError::MultipleKeys);
            }
            key = Some(parsed);
        }
        let key = key.ok_or(ChordParseError::NoKey)?;
        Ok(Chord { mods, key })
    }

    pub fn from_event(event: &KeyEvent, modifiers: &Modifiers) -> Option<Chord> {
        Some(Chord {
            mods: mods_from_state(modifiers.state()),
            key: key_token_from_logical(&event.logical_key)?,
        })
    }
}

fn modifier_bit(tok: &str) -> Option<ModSet> {
    Some(match tok {
        "ctrl" | "control" => ModSet::CTRL,
        "alt" | "opt" | "option" => ModSet::ALT,
        "shift" => ModSet::SHIFT,
        "cmd" | "command" | "super" | "meta" | "win" | "windows" => ModSet::SUPER,
        _ => return None,
    })
}

fn parse_key_token(tok: &str) -> Option<KeyToken> {
    let mut chars = tok.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(normalize_char(first));
    }
    Some(match tok {
        "enter" | "return" => KeyToken::Named(NamedToken::Enter),
        "escape" | "esc" => KeyToken::Named(NamedToken::Escape),
        "tab" => KeyToken::Named(NamedToken::Tab),
        "space" => KeyToken::Named(NamedToken::Space),
        "backspace" => KeyToken::Named(NamedToken::Backspace),
        "delete" | "del" => KeyToken::Named(NamedToken::Delete),
        "minus" => KeyToken::Named(NamedToken::Minus),
        // `plus` folds onto `equal`: they are the same physical key, and a
        // shift-produced `+` and a bare `=` must resolve to one binding.
        "plus" | "equal" | "equals" => KeyToken::Named(NamedToken::Equal),
        "up" | "arrowup" => KeyToken::Named(NamedToken::Up),
        "down" | "arrowdown" => KeyToken::Named(NamedToken::Down),
        "left" | "arrowleft" => KeyToken::Named(NamedToken::Left),
        "right" | "arrowright" => KeyToken::Named(NamedToken::Right),
        "home" => KeyToken::Named(NamedToken::Home),
        "end" => KeyToken::Named(NamedToken::End),
        "pageup" | "pgup" => KeyToken::Named(NamedToken::PageUp),
        "pagedown" | "pgdn" => KeyToken::Named(NamedToken::PageDown),
        _ => KeyToken::Named(NamedToken::F(fkey(tok)?)),
    })
}

fn fkey(tok: &str) -> Option<u8> {
    let n: u8 = tok.strip_prefix('f')?.parse().ok()?;
    (1..=24).contains(&n).then_some(n)
}

/// Fold a produced character to its canonical token. `+`/`=` collapse to
/// `Equal` and `-` to `Minus` so the spelled names and the produced glyphs
/// resolve to the same binding regardless of which side the shift was on.
fn normalize_char(c: char) -> KeyToken {
    match c.to_ascii_lowercase() {
        '+' | '=' => KeyToken::Named(NamedToken::Equal),
        '-' => KeyToken::Named(NamedToken::Minus),
        other => KeyToken::Char(other),
    }
}

fn mods_from_state(state: ModifiersState) -> ModSet {
    let mut mods = ModSet::empty();
    if state.shift_key() {
        mods.insert(ModSet::SHIFT);
    }
    if state.control_key() {
        mods.insert(ModSet::CTRL);
    }
    if state.alt_key() {
        mods.insert(ModSet::ALT);
    }
    if state.super_key() {
        mods.insert(ModSet::SUPER);
    }
    mods
}

fn key_token_from_logical(key: &Key) -> Option<KeyToken> {
    match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            chars.next().is_none().then(|| normalize_char(c))
        }
        Key::Named(named) => named_token(*named).map(KeyToken::Named),
        _ => None,
    }
}

fn named_token(named: NamedKey) -> Option<NamedToken> {
    use NamedKey as N;
    use NamedToken as T;
    Some(match named {
        N::Enter => T::Enter,
        N::Escape => T::Escape,
        N::Tab => T::Tab,
        N::Space => T::Space,
        N::Backspace => T::Backspace,
        N::Delete => T::Delete,
        N::ArrowUp => T::Up,
        N::ArrowDown => T::Down,
        N::ArrowLeft => T::Left,
        N::ArrowRight => T::Right,
        N::Home => T::Home,
        N::End => T::End,
        N::PageUp => T::PageUp,
        N::PageDown => T::PageDown,
        N::F1 => T::F(1),
        N::F2 => T::F(2),
        N::F3 => T::F(3),
        N::F4 => T::F(4),
        N::F5 => T::F(5),
        N::F6 => T::F(6),
        N::F7 => T::F(7),
        N::F8 => T::F(8),
        N::F9 => T::F(9),
        N::F10 => T::F(10),
        N::F11 => T::F(11),
        N::F12 => T::F(12),
        N::F13 => T::F(13),
        N::F14 => T::F(14),
        N::F15 => T::F(15),
        N::F16 => T::F(16),
        N::F17 => T::F(17),
        N::F18 => T::F(18),
        N::F19 => T::F(19),
        N::F20 => T::F(20),
        N::F21 => T::F(21),
        N::F22 => T::F(22),
        N::F23 => T::F(23),
        N::F24 => T::F(24),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_of(spec: &str) -> KeyToken {
        Chord::parse(spec).unwrap().key
    }

    #[test]
    fn modifier_aliases_are_equivalent() {
        let canonical = Chord::parse("cmd+c").unwrap();
        for alias in ["super+c", "meta+c", "win+c", "command+c", "windows+c"] {
            assert_eq!(Chord::parse(alias).unwrap(), canonical, "{alias}");
        }
        let alt = Chord::parse("alt+a").unwrap();
        assert_eq!(Chord::parse("opt+a").unwrap(), alt);
        assert_eq!(Chord::parse("option+a").unwrap(), alt);
    }

    #[test]
    fn modifiers_accumulate() {
        let chord = Chord::parse("ctrl+shift+c").unwrap();
        assert_eq!(chord.mods, ModSet::CTRL | ModSet::SHIFT);
        assert_eq!(chord.key, KeyToken::Char('c'));
    }

    #[test]
    fn named_keys_parse() {
        assert_eq!(key_of("cmd+enter"), KeyToken::Named(NamedToken::Enter));
        assert_eq!(key_of("escape"), KeyToken::Named(NamedToken::Escape));
        assert_eq!(key_of("f5"), KeyToken::Named(NamedToken::F(5)));
        assert_eq!(key_of("up"), KeyToken::Named(NamedToken::Up));
        assert_eq!(key_of("space"), KeyToken::Named(NamedToken::Space));
    }

    #[test]
    fn plus_and_equal_fold_together() {
        assert_eq!(Chord::parse("cmd+plus"), Chord::parse("cmd+equal"));
        assert_eq!(key_of("cmd+plus"), KeyToken::Named(NamedToken::Equal));
    }

    #[test]
    fn parse_errors() {
        assert_eq!(Chord::parse("").unwrap_err(), ChordParseError::Empty);
        assert_eq!(Chord::parse("   ").unwrap_err(), ChordParseError::Empty);
        assert_eq!(Chord::parse("ctrl+").unwrap_err(), ChordParseError::NoKey);
        assert_eq!(
            Chord::parse("a+b").unwrap_err(),
            ChordParseError::MultipleKeys
        );
        assert_eq!(
            Chord::parse("ctrl+blah").unwrap_err(),
            ChordParseError::UnknownToken("blah".to_string())
        );
        assert_eq!(
            Chord::parse("f99").unwrap_err(),
            ChordParseError::UnknownToken("f99".to_string())
        );
    }

    #[test]
    fn mods_from_state_maps_each_bit() {
        let state = ModifiersState::SUPER | ModifiersState::SHIFT;
        assert_eq!(mods_from_state(state), ModSet::SUPER | ModSet::SHIFT);
        assert_eq!(mods_from_state(ModifiersState::empty()), ModSet::empty());
    }

    #[test]
    fn logical_key_mapping_matches_parse() {
        assert_eq!(
            key_token_from_logical(&Key::Character("c".into())),
            Some(KeyToken::Char('c'))
        );
        // A produced `+` and a produced `=` both fold to the parsed `plus`.
        assert_eq!(
            key_token_from_logical(&Key::Character("+".into())),
            Some(key_of("cmd+plus"))
        );
        assert_eq!(
            key_token_from_logical(&Key::Character("=".into())),
            Some(key_of("cmd+equal"))
        );
        assert_eq!(
            key_token_from_logical(&Key::Named(NamedKey::Enter)),
            Some(KeyToken::Named(NamedToken::Enter))
        );
        assert_eq!(
            key_token_from_logical(&Key::Named(NamedKey::F2)),
            Some(key_of("f2"))
        );
    }
}
