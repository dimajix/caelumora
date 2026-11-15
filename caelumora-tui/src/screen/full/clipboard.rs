//! The clipboard and the primary selection what the user marks in the full-screen terminal is
//! copied to (issue 576).
//!
//! Where there is a display, they are the system's, reached through arboard; where there is
//! none - over SSH -, the terminal's, which the program tells by OSC 52 and which a terminal may
//! refuse.

// Where arboard offers the primary selection: the systems of X11 and Wayland.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
))]
use arboard::{LinuxClipboardKind, SetExtLinux};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use crossterm::execute;
use crossterm::style::Print;
use std::io;
use tracing::{debug, warn};

/// The clipboard and the primary selection what is marked is copied to: the system's, reached
/// through arboard where there is a display, else the terminal's, by OSC 52.
pub(super) enum Clipboard {
    /// The system's, where there is a display.
    System(arboard::Clipboard),
    /// The terminal's, told by OSC 52.
    Terminal,
}

impl Clipboard {
    /// The system's clipboard where it can be reached, else the terminal's.
    pub(super) fn new() -> Self {
        match arboard::Clipboard::new() {
            Ok(clipboard) => Self::System(clipboard),
            Err(error) => {
                debug!("The system's clipboard cannot be reached, the terminal's is used: {error}");
                Self::Terminal
            }
        }
    }

    /// Puts `text` into the clipboard and the primary selection; what fails is logged.
    pub(super) fn copy(&mut self, text: &str) {
        match self {
            Self::System(clipboard) => {
                if let Err(error) = copy_to_system(clipboard, text) {
                    warn!("The system's clipboard did not take what was marked: {error}");
                }
            }
            Self::Terminal => {
                if let Err(error) = execute!(io::stdout(), Print(osc52(text))) {
                    warn!("The terminal could not be told what was marked: {error}");
                }
            }
        }
    }
}

/// Puts `text` into the system's `clipboard` and, where it has one - with X11 or Wayland -, its
/// primary selection.
fn copy_to_system(clipboard: &mut arboard::Clipboard, text: &str) -> Result<(), arboard::Error> {
    clipboard.set_text(text)?;
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
    ))]
    clipboard.set().clipboard(LinuxClipboardKind::Primary).text(text)?;
    Ok(())
}

/// What tells a terminal by OSC 52 to put `text` into its clipboard (`c`) and its primary
/// selection (`p`) - one sequence each, since not every terminal takes both from one.
fn osc52(text: &str) -> String {
    let encoded = BASE64.encode(text);
    format!("\x1b]52;c;{encoded}\x07\x1b]52;p;{encoded}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text is told base64-encoded, to the clipboard and to the primary selection.
    #[test]
    fn osc52_tells_the_clipboard_and_the_primary_selection() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07\x1b]52;p;aGk=\x07");
    }
}
