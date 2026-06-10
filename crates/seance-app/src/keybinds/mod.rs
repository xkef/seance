//! Global keybinds that take precedence over VT input encoding.
//!
//! A data-driven table maps a [`Chord`] (modifier set + key token) to an
//! [`Action`]. Built-in defaults are seeded first; user `[[keybind]]` entries
//! override by chord, and an `unbind` action removes one. Consulted before the
//! VT encoder; on a miss the event falls through to `seance_input::InputHandler`.

mod action;
mod chord;

use std::collections::HashMap;

use winit::event::{ElementState, KeyEvent, Modifiers};

use seance_config::KeybindConfig;

pub use action::Action;
use chord::{Chord, KeyToken, ModSet, NamedToken};

pub struct Keybinds {
    table: HashMap<Chord, Action>,
}

impl Keybinds {
    pub fn from_config(binds: &[KeybindConfig]) -> Self {
        let mut table = Self::defaults();
        for bind in binds {
            let chord = match Chord::parse(&bind.key) {
                Ok(chord) => chord,
                Err(err) => {
                    tracing::warn!("ignoring keybind {:?}: invalid key ({err:?})", bind.key);
                    continue;
                }
            };
            match action::ActionSpec::parse(&bind.action) {
                Ok(action::ActionSpec::Bind(action)) => {
                    table.insert(chord, action);
                }
                Ok(action::ActionSpec::Unbind) => {
                    table.remove(&chord);
                }
                Err(err) => {
                    tracing::warn!(
                        "ignoring keybind {:?}: invalid action {:?} ({err:?})",
                        bind.key,
                        bind.action
                    );
                }
            }
        }
        Self { table }
    }

    pub fn match_event(&self, event: &KeyEvent, modifiers: &Modifiers) -> Option<Action> {
        if event.state != ElementState::Pressed {
            return None;
        }
        let chord = Chord::from_event(event, modifiers)?;
        self.table.get(&chord).copied()
    }

    fn defaults() -> HashMap<Chord, Action> {
        let sup = ModSet::SUPER;
        let named = |n: NamedToken| KeyToken::Named(n);
        [
            (Chord::new(sup, KeyToken::Char('q')), Action::Quit),
            (Chord::new(sup, KeyToken::Char('w')), Action::CloseSurface),
            (Chord::new(sup, KeyToken::Char('c')), Action::Copy),
            (Chord::new(sup, KeyToken::Char('v')), Action::Paste),
            (Chord::new(sup, KeyToken::Char('a')), Action::SelectAll),
            (
                Chord::new(sup, named(NamedToken::Equal)),
                Action::FontSize(1),
            ),
            (
                Chord::new(sup, named(NamedToken::Minus)),
                Action::FontSize(-1),
            ),
            (Chord::new(sup, KeyToken::Char('0')), Action::ResetFontSize),
            (
                Chord::new(sup, named(NamedToken::Enter)),
                Action::ToggleFullscreen,
            ),
        ]
        .into_iter()
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind(key: &str, action: &str) -> KeybindConfig {
        KeybindConfig {
            key: key.to_string(),
            action: action.to_string(),
        }
    }

    fn lookup(binds: &Keybinds, spec: &str) -> Option<Action> {
        binds.table.get(&Chord::parse(spec).unwrap()).copied()
    }

    #[test]
    fn empty_config_equals_defaults() {
        let binds = Keybinds::from_config(&[]);
        assert_eq!(binds.table, Keybinds::defaults());
        assert_eq!(lookup(&binds, "cmd+c"), Some(Action::Copy));
        assert_eq!(lookup(&binds, "cmd+q"), Some(Action::Quit));
        assert_eq!(lookup(&binds, "cmd+plus"), Some(Action::FontSize(1)));
        assert_eq!(lookup(&binds, "cmd+enter"), Some(Action::ToggleFullscreen));
    }

    #[test]
    fn user_entry_overrides_default() {
        let binds = Keybinds::from_config(&[bind("cmd+c", "paste")]);
        assert_eq!(lookup(&binds, "cmd+c"), Some(Action::Paste));
    }

    #[test]
    fn unbind_removes_default() {
        let binds = Keybinds::from_config(&[bind("cmd+q", "unbind")]);
        assert_eq!(lookup(&binds, "cmd+q"), None);
        assert_eq!(lookup(&binds, "cmd+c"), Some(Action::Copy));
    }

    #[test]
    fn non_super_binding_resolves() {
        let binds = Keybinds::from_config(&[bind("ctrl+shift+c", "copy")]);
        assert_eq!(lookup(&binds, "ctrl+shift+c"), Some(Action::Copy));
    }

    #[test]
    fn malformed_entry_is_skipped_without_poisoning_table() {
        let binds = Keybinds::from_config(&[
            bind("ctrl+nonsense", "copy"),
            bind("cmd+c", "bogus_action"),
            bind("ctrl+shift+t", "new_tab"),
        ]);
        assert_eq!(lookup(&binds, "cmd+c"), Some(Action::Copy));
        assert_eq!(lookup(&binds, "ctrl+shift+t"), Some(Action::NewTab));
    }
}
