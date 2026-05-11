//! Terminal-color helpers for `dop` output.
//!
//! # Scheme
//!
//! The color scheme matches ledger-cli's `bal --force-color` output:
//! - **Account names** are rendered in blue (ANSI color 34).
//! - **Negative amounts** are rendered in red (ANSI color 31).
//! - **Positive amounts and the separator line** are unstyled.
//!
//! # Detection
//!
//! Color is controlled by the `--color` global flag (`auto` / `always` / `never`):
//! - `auto` (default): emit color when stdout is a TTY *and* the `NO_COLOR`
//!   environment variable is unset or empty, per <https://no-color.org/>.
//! - `always`: always emit ANSI escapes even when piped/redirected.
//! - `never`: always suppress color.
//!
//! Snapshot tests run with stdout captured (not a TTY), so `auto` naturally
//! yields plain ASCII there — no changes to existing snapshots are needed.

use anstyle::{AnsiColor, Style};

/// The resolved color mode after combining `--color`, `NO_COLOR`, and TTY state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Emit ANSI color codes.
    On,
    /// Emit plain ASCII only.
    Off,
}

impl ColorMode {
    /// Render an amount right-aligned to `width` display columns, with red ANSI
    /// styling around the content if this mode is `On` and `is_negative` is true.
    ///
    /// `s` is the pre-formatted display string (e.g. `"$ -2,000.00"`).
    /// `is_negative` is derived from the underlying decimal value so that
    /// commodity-prefix formats (where the `-` is not the first character) are
    /// handled correctly.
    ///
    /// Leading spaces (the right-align padding) are emitted outside the ANSI
    /// codes, matching ledger-cli's visual layout.
    pub fn render_amount(&self, s: &str, width: usize, is_negative: bool) -> String {
        let content_len = s.chars().count();
        let pad = width.saturating_sub(content_len);
        let spaces = " ".repeat(pad);
        if *self == ColorMode::On && is_negative {
            let style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red)));
            format!("{spaces}{style}{s}{style:#}")
        } else {
            format!("{spaces}{s}")
        }
    }

    /// Wrap an account name string with blue ANSI styling if this mode is `On`.
    pub fn style_account<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        if *self == ColorMode::Off {
            return std::borrow::Cow::Borrowed(s);
        }
        let style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Blue)));
        std::borrow::Cow::Owned(format!("{style}{s}{style:#}"))
    }
}
