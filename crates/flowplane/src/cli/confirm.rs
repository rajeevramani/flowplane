//! Destructive-action confirmation (CLI-R-22/26).
//!
//! A destructive command (`delete`, `unexpose`) prompts `[y/N]` on an interactive terminal;
//! `--yes` skips the prompt; on a non-interactive terminal **without** `--yes` it fails fast
//! (exit 2) and never blocks reading stdin. The decision is split from the IO so every branch
//! is unit-testable. (`apply` is additive-only and `apply --prune` is rejected as unsupported,
//! so there is no prune surface to confirm.)

use crate::cli::config::GlobalOptions;
use crate::cli::output::CliError;
use anyhow::Result;
use std::io::{IsTerminal, Write};

/// What to do for a destructive action, given the flags, terminal state, and the answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfirmOutcome {
    /// `--yes`, or the user typed yes at the prompt.
    Proceed,
    /// Interactive, user declined (the safe default for a bare Enter / anything but yes).
    Declined,
    /// Non-interactive and no `--yes`: refuse without ever reading stdin (CLI-R-26).
    NonInteractive,
}

/// Pure confirmation decision (CLI-R-22/26). `answer` is the line read from stdin, only when
/// interactive; `None` when non-interactive (it must NOT be read in that case).
pub(crate) fn decide(yes: bool, stdin_is_tty: bool, answer: Option<&str>) -> ConfirmOutcome {
    if yes {
        return ConfirmOutcome::Proceed;
    }
    if !stdin_is_tty {
        return ConfirmOutcome::NonInteractive;
    }
    match answer.map(|a| a.trim().to_ascii_lowercase()) {
        Some(a) if a == "y" || a == "yes" => ConfirmOutcome::Proceed,
        _ => ConfirmOutcome::Declined,
    }
}

/// Confirm a destructive `action` (e.g. `delete clusters/alpha`). Returns `Ok(())` to proceed;
/// otherwise an already-reported [`CliError`] with the right exit code (declined → 1,
/// non-interactive without `--yes` → 2 usage). Reads stdin only when interactive.
pub(crate) fn confirm_destructive(global: &GlobalOptions, action: &str) -> Result<()> {
    let stdin_is_tty = std::io::stdin().is_terminal();
    // Decide first WITHOUT reading stdin for the trivial cases (CLI-R-26: never block on a
    // non-interactive terminal).
    if global.yes {
        return Ok(());
    }
    if !stdin_is_tty {
        eprintln!(
            "error (confirmation_required): refusing to {action} without --yes on a \
             non-interactive terminal; pass --yes to confirm"
        );
        return Err(anyhow::Error::new(CliError::new(2)));
    }
    eprint!("{action}? [y/N]: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    match decide(false, true, Some(&line)) {
        ConfirmOutcome::Proceed => Ok(()),
        _ => {
            eprintln!("aborted");
            Err(anyhow::Error::new(CliError::new(1)))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn yes_flag_proceeds_regardless_of_tty_or_answer() {
        assert_eq!(decide(true, true, Some("n")), ConfirmOutcome::Proceed);
        assert_eq!(decide(true, false, None), ConfirmOutcome::Proceed);
    }

    #[test]
    fn non_interactive_without_yes_is_fail_fast_never_reads() {
        // answer is None — the IO layer must not have read stdin.
        assert_eq!(decide(false, false, None), ConfirmOutcome::NonInteractive);
    }

    #[test]
    fn interactive_prompt_yes_no_and_default() {
        assert_eq!(decide(false, true, Some("y\n")), ConfirmOutcome::Proceed);
        assert_eq!(decide(false, true, Some("yes\n")), ConfirmOutcome::Proceed);
        assert_eq!(decide(false, true, Some("Y")), ConfirmOutcome::Proceed);
        // bare Enter / anything else → declined (safe default N).
        assert_eq!(decide(false, true, Some("\n")), ConfirmOutcome::Declined);
        assert_eq!(decide(false, true, Some("n")), ConfirmOutcome::Declined);
        assert_eq!(
            decide(false, true, Some("nonsense")),
            ConfirmOutcome::Declined
        );
    }
}
