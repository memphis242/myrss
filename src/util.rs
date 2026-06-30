//! miscellaneous functions that feel like they don't fit anywhere else

use ratatui::widgets::ListState;

/// Strips terminal control characters from text that originates from untrusted
/// feeds before it is rendered to the terminal, preventing ANSI/OSC escape
/// sequence injection (e.g. a feed title containing `\x1b]0;…` to rewrite the
/// window title, or `\x1b[2J` to clear the screen).
///
/// Newline and tab are preserved because the renderer relies on them for layout;
/// every other C0/C1 control character (Unicode category `Cc`, which includes
/// `ESC`, `BEL`, and carriage return) is removed.
pub fn sanitize_terminal_text(input: &str) -> String {
    input
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect()
}

#[derive(Debug)]
pub struct StatefulList<T> {
    pub state: ListState,
    pub items: Vec<T>,
}

impl<T> StatefulList<T> {
    pub fn with_items(items: Vec<T>) -> StatefulList<T> {
        StatefulList {
            state: ListState::default(),
            items,
        }
    }

    pub fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.items.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    pub fn previous(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    pub fn reset(&mut self) {
        self.state.select(Some(0));
    }

    pub fn unselect(&mut self) {
        self.state.select(None);
    }

    pub fn snap_to_top(&mut self) {
        if !self.items.is_empty() {
            self.state.select(Some(0));
        }
    }

    pub fn snap_to_bottom(&mut self) {
        if !self.items.is_empty() {
            self.state.select(Some(self.items.len() - 1));
        }
    }
}

impl<T> From<Vec<T>> for StatefulList<T> {
    fn from(other: Vec<T>) -> Self {
        StatefulList::with_items(other)
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn set_wsl_clipboard_contents(s: &str) -> anyhow::Result<()> {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    // it looks like this on the CLI:
    // `echo "foo" | clip.exe`
    let mut clipboard = Command::new("clip.exe").stdin(Stdio::piped()).spawn()?;

    let mut clipboard_stdin = clipboard
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Unable to get stdin handle for clip.exe"))?;

    clipboard_stdin.write_all(s.as_bytes())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stateful_list_snap() {
        let mut list = StatefulList::with_items(vec![10, 20, 30, 40]);
        assert_eq!(list.state.selected(), None);

        list.snap_to_top();
        assert_eq!(list.state.selected(), Some(0));

        list.next();
        assert_eq!(list.state.selected(), Some(1));

        list.snap_to_bottom();
        assert_eq!(list.state.selected(), Some(3));
    }

    #[test]
    fn test_sanitize_terminal_text() {
        // ESC, BEL, and C1 controls are stripped; newline and tab survive.
        let input = "safe\u{1b}[31mred\u{7}\ttab\nnewline\u{9b}";
        let out = sanitize_terminal_text(input);
        assert_eq!(out, "safe[31mred\ttab\nnewline");
        assert!(!out.contains('\u{1b}'));
        assert!(!out.contains('\u{7}'));
        assert!(!out.contains('\u{9b}'));
        assert!(out.contains('\t'));
        assert!(out.contains('\n'));
    }
}
