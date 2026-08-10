//! OSC 133 semantic prompt parsing (FinalTerm / iTerm2 shell integration).
//!
//! A shell with integration hooks brackets each prompt and command with:
//!
//! - `ESC ] 133 ; A ST` — prompt start
//! - `ESC ] 133 ; B ST` — prompt end / command input start
//! - `ESC ] 133 ; C ST` — command output start (pre-exec finished)
//! - `ESC ] 133 ; D [ ; <exit> ] ST` — command finished, optional exit code
//!
//! Emitters append implementation-defined `;key=value` parameters after the
//! marker letter (ghostty/kitty attach `aid=`, `k=`); the parser tolerates and
//! ignores them. Only the exit code on the `D` marker feeds terminal state
//! today; the full marker set is surfaced so a future consumer (Lua
//! `seance.event`, prompt-aware scrolling) can react without re-parsing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticPrompt {
    PromptStart,
    PromptEnd,
    CommandStart,
    CommandEnd { exit: Option<i32> },
}

/// Parse the body of an OSC command (the bytes between `OSC` and the
/// terminator) as an OSC 133 semantic prompt marker. Returns `None` for any
/// other OSC code or an unrecognized marker letter.
pub(crate) fn parse(content: &[u8]) -> Option<SemanticPrompt> {
    let body = content.strip_prefix(b"133;")?;
    let mut fields = body.split(|&b| b == b';');
    let marker = fields.next()?;
    match marker {
        b"A" => Some(SemanticPrompt::PromptStart),
        b"B" => Some(SemanticPrompt::PromptEnd),
        b"C" => Some(SemanticPrompt::CommandStart),
        b"D" => Some(SemanticPrompt::CommandEnd {
            exit: fields.next().and_then(parse_exit_code),
        }),
        _ => None,
    }
}

/// The exit code field is decimal ASCII. An empty or non-numeric field (the
/// shell finished a command it can report no status for, e.g. the very first
/// prompt) decodes to `None` rather than a parse failure.
fn parse_exit_code(field: &[u8]) -> Option<i32> {
    std::str::from_utf8(field).ok()?.trim().parse::<i32>().ok()
}

/// Per-terminal semantic prompt state derived from the OSC 133 marker stream.
///
/// `last_command_exit` is the exit code reported by the most recent `D`
/// marker — the shell-command status, distinct from the pane process's own
/// exit. `None` before any command completes, or when a `D` marker carries no
/// code.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PromptState {
    pub(crate) last_command_exit: Option<i32>,
}

impl PromptState {
    pub(crate) fn apply(&mut self, event: SemanticPrompt) {
        if let SemanticPrompt::CommandEnd { exit } = event {
            self.last_command_exit = exit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_markers() {
        assert_eq!(parse(b"133;A"), Some(SemanticPrompt::PromptStart));
        assert_eq!(parse(b"133;B"), Some(SemanticPrompt::PromptEnd));
        assert_eq!(parse(b"133;C"), Some(SemanticPrompt::CommandStart));
        assert_eq!(
            parse(b"133;D"),
            Some(SemanticPrompt::CommandEnd { exit: None })
        );
    }

    #[test]
    fn parses_command_end_exit_code() {
        assert_eq!(
            parse(b"133;D;0"),
            Some(SemanticPrompt::CommandEnd { exit: Some(0) })
        );
        assert_eq!(
            parse(b"133;D;130"),
            Some(SemanticPrompt::CommandEnd { exit: Some(130) })
        );
    }

    #[test]
    fn command_end_with_empty_exit_is_none() {
        assert_eq!(
            parse(b"133;D;"),
            Some(SemanticPrompt::CommandEnd { exit: None })
        );
    }

    #[test]
    fn ignores_trailing_parameters() {
        assert_eq!(parse(b"133;A;aid=1"), Some(SemanticPrompt::PromptStart));
        assert_eq!(
            parse(b"133;D;1;aid=7"),
            Some(SemanticPrompt::CommandEnd { exit: Some(1) })
        );
    }

    #[test]
    fn rejects_non_133_and_unknown_markers() {
        assert_eq!(parse(b"7;file:///tmp"), None);
        assert_eq!(parse(b"52;c;?"), None);
        assert_eq!(parse(b"133;Z"), None);
        assert_eq!(parse(b"133;"), None);
    }

    #[test]
    fn tracker_records_latest_command_exit() {
        let mut state = PromptState::default();
        assert_eq!(state.last_command_exit, None);

        state.apply(SemanticPrompt::PromptStart);
        state.apply(SemanticPrompt::CommandStart);
        assert_eq!(state.last_command_exit, None);

        state.apply(SemanticPrompt::CommandEnd { exit: Some(1) });
        assert_eq!(state.last_command_exit, Some(1));

        state.apply(SemanticPrompt::CommandEnd { exit: Some(0) });
        assert_eq!(state.last_command_exit, Some(0));
    }

    #[test]
    fn tracker_clears_on_uncoded_command_end() {
        let mut state = PromptState {
            last_command_exit: Some(2),
        };
        state.apply(SemanticPrompt::CommandEnd { exit: None });
        assert_eq!(state.last_command_exit, None);
    }
}
