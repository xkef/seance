//! The config-facing action vocabulary and its string grammar.
//!
//! `Action` is the full set from the keybind spec; `to_app_command` projects
//! the subset the app can dispatch today onto [`AppCommand`]. The remaining
//! (mux) actions parse successfully and are logged as not-yet-implemented when
//! triggered, so a config written against the final vocabulary loads cleanly.

use crate::command::AppCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Copy,
    Paste,
    SelectAll,
    Quit,
    CloseSurface,
    FontSize(i8),
    ResetFontSize,
    ToggleFullscreen,
    NewTab,
    NewWindow,
    SplitH,
    SplitV,
    FocusPane(Direction),
    SwitchTab(i32),
    Scroll(Direction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionSpec {
    Bind(Action),
    Unbind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionParseError {
    Unknown(String),
    MissingArg(String),
    BadArg(String),
}

impl Action {
    pub fn to_app_command(self) -> Option<AppCommand> {
        Some(match self {
            Action::Copy => AppCommand::Copy,
            Action::Paste => AppCommand::Paste,
            Action::SelectAll => AppCommand::SelectAll,
            Action::Quit => AppCommand::Quit,
            Action::CloseSurface => AppCommand::CloseWindow,
            Action::FontSize(delta) => AppCommand::FontSizeDelta(delta),
            Action::ResetFontSize => AppCommand::FontSizeReset,
            Action::ToggleFullscreen => AppCommand::ToggleFullscreen,
            Action::NewTab
            | Action::NewWindow
            | Action::SplitH
            | Action::SplitV
            | Action::FocusPane(_)
            | Action::SwitchTab(_)
            | Action::Scroll(_) => return None,
        })
    }
}

impl ActionSpec {
    pub fn parse(spec: &str) -> Result<ActionSpec, ActionParseError> {
        let (name, arg) = match spec.trim().split_once(':') {
            Some((name, arg)) => (name.trim(), Some(arg.trim())),
            None => (spec.trim(), None),
        };
        let name = name.to_ascii_lowercase();
        let action = match name.as_str() {
            "unbind" | "ignore" | "none" => return Ok(ActionSpec::Unbind),
            "copy" => Action::Copy,
            "paste" => Action::Paste,
            "select_all" => Action::SelectAll,
            "quit" => Action::Quit,
            "close_surface" => Action::CloseSurface,
            "reset_font_size" => Action::ResetFontSize,
            "toggle_fullscreen" => Action::ToggleFullscreen,
            "new_tab" => Action::NewTab,
            "new_window" => Action::NewWindow,
            "split_h" => Action::SplitH,
            "split_v" => Action::SplitV,
            "font_size" | "font_size_inc" => Action::FontSize(parse_i8(&name, arg)?),
            "font_size_dec" => Action::FontSize(parse_i8(&name, arg)?.saturating_neg()),
            "switch_tab" => Action::SwitchTab(parse_int(&name, arg)?),
            "focus_pane" => Action::FocusPane(parse_dir(&name, arg)?),
            "scroll" => Action::Scroll(parse_dir(&name, arg)?),
            _ => return Err(ActionParseError::Unknown(name)),
        };
        Ok(ActionSpec::Bind(action))
    }
}

fn parse_i8(name: &str, arg: Option<&str>) -> Result<i8, ActionParseError> {
    let arg = arg.ok_or_else(|| ActionParseError::MissingArg(name.to_string()))?;
    arg.parse()
        .map_err(|_| ActionParseError::BadArg(arg.to_string()))
}

fn parse_int(name: &str, arg: Option<&str>) -> Result<i32, ActionParseError> {
    let arg = arg.ok_or_else(|| ActionParseError::MissingArg(name.to_string()))?;
    arg.parse()
        .map_err(|_| ActionParseError::BadArg(arg.to_string()))
}

fn parse_dir(name: &str, arg: Option<&str>) -> Result<Direction, ActionParseError> {
    let arg = arg.ok_or_else(|| ActionParseError::MissingArg(name.to_string()))?;
    match arg {
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        _ => Err(ActionParseError::BadArg(arg.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind(spec: &str) -> Action {
        match ActionSpec::parse(spec).unwrap() {
            ActionSpec::Bind(a) => a,
            ActionSpec::Unbind => panic!("expected bind for {spec}"),
        }
    }

    #[test]
    fn simple_names_parse() {
        assert_eq!(bind("copy"), Action::Copy);
        assert_eq!(bind("paste"), Action::Paste);
        assert_eq!(bind("select_all"), Action::SelectAll);
        assert_eq!(bind("quit"), Action::Quit);
        assert_eq!(bind("toggle_fullscreen"), Action::ToggleFullscreen);
        assert_eq!(bind("close_surface"), Action::CloseSurface);
    }

    #[test]
    fn arg_forms_parse() {
        assert_eq!(bind("font_size_inc:1"), Action::FontSize(1));
        assert_eq!(bind("font_size_dec:2"), Action::FontSize(-2));
        assert_eq!(bind("font_size:-3"), Action::FontSize(-3));
        assert_eq!(bind("switch_tab:3"), Action::SwitchTab(3));
        assert_eq!(bind("focus_pane:left"), Action::FocusPane(Direction::Left));
        assert_eq!(bind("scroll:up"), Action::Scroll(Direction::Up));
    }

    #[test]
    fn unbind_aliases_parse() {
        for spec in ["unbind", "ignore", "none"] {
            assert_eq!(
                ActionSpec::parse(spec).unwrap(),
                ActionSpec::Unbind,
                "{spec}"
            );
        }
    }

    #[test]
    fn parse_errors() {
        assert_eq!(
            ActionSpec::parse("font_size_inc").unwrap_err(),
            ActionParseError::MissingArg("font_size_inc".to_string())
        );
        assert_eq!(
            ActionSpec::parse("font_size_inc:x").unwrap_err(),
            ActionParseError::BadArg("x".to_string())
        );
        assert_eq!(
            ActionSpec::parse("focus_pane:sideways").unwrap_err(),
            ActionParseError::BadArg("sideways".to_string())
        );
        assert_eq!(
            ActionSpec::parse("bogus").unwrap_err(),
            ActionParseError::Unknown("bogus".to_string())
        );
    }

    #[test]
    fn mux_actions_have_no_app_command() {
        assert!(Action::NewTab.to_app_command().is_none());
        assert!(Action::SplitV.to_app_command().is_none());
        assert!(Action::FocusPane(Direction::Up).to_app_command().is_none());
        assert!(Action::Copy.to_app_command().is_some());
    }
}
