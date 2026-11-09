//! ANSI colour helpers for console output.
//!
//! Colour is dropped when stdout is not a terminal, so piped logs and captured
//! test output stay plain text.

use std::io::IsTerminal;

/// Colour is off when the output is piped or when `NO_COLOR` is set.
fn use_color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn render(enabled: bool, code: &str, text: &str) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn paint(code: &str, text: &str) -> String {
    render(use_color(), code, text)
}

pub fn green(text: &str) -> String {
    paint("32", text)
}

pub fn red(text: &str) -> String {
    paint("31", text)
}

pub fn yellow(text: &str) -> String {
    paint("33", text)
}

pub fn cyan(text: &str) -> String {
    paint("36", text)
}

pub fn dim(text: &str) -> String {
    paint("90", text)
}

pub fn bold(text: &str) -> String {
    paint("1", text)
}

/// Green for a gain, red for a loss, dim for flat.
pub fn signed(value: f64, text: &str) -> String {
    if value > 0.0 {
        green(text)
    } else if value < 0.0 {
        red(text)
    } else {
        dim(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_codes_are_emitted_only_when_enabled() {
        assert_eq!(render(true, "32", "up"), "\x1b[32mup\x1b[0m");
        assert_eq!(render(false, "32", "up"), "up");
    }

    #[test]
    fn colour_is_dropped_when_not_a_terminal() {
        // Tests capture stdout, so nothing should carry escape codes.
        assert_eq!(green("+1.00%"), "+1.00%");
        assert_eq!(red("-1.00%"), "-1.00%");
        assert_eq!(signed(2.0, "+2.00"), "+2.00");
        assert_eq!(signed(-2.0, "-2.00"), "-2.00");
        assert_eq!(signed(0.0, "0.00"), "0.00");
    }
}
