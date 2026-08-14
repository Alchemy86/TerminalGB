//! Opening the sign-in page — a convenience, never a requirement.
//!
//! The captain's ask is the Claude Code `/login` shape: trigger sign-in from
//! inside the emulator and have the browser open itself. But a headless box, an
//! SSH session or a machine with no opener must still sign in, so the URL and
//! code are **always printed/shown as well**, and every failure here is soft:
//! the return value only decides which wording the user sees.

use std::process::{Command, Stdio};

/// Try to open `url` in the user's browser. Returns whether an opener was
/// launched — not proof a page appeared, which is why the caller must show
/// the URL regardless.
pub fn open(url: &str) -> bool {
    if std::env::var("GB_PORTAL_NO_BROWSER").is_ok_and(|v| v != "0") {
        return false;
    }
    for mut cmd in candidates(url) {
        if cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
        {
            return true;
        }
    }
    false
}

/// The platform-appropriate openers, most specific first. `$BROWSER` wins so
/// the user's choice is honoured (and so a test can point it at `true`).
fn candidates(url: &str) -> Vec<Command> {
    let mut out = Vec::new();
    if let Ok(browser) = std::env::var("BROWSER") {
        let browser = browser.trim();
        if !browser.is_empty() {
            let mut c = Command::new(browser);
            c.arg(url);
            out.push(c);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("open");
        c.arg(url);
        out.push(c);
    }
    #[cfg(target_os = "windows")]
    {
        let mut c = Command::new("cmd");
        // The empty "" is start's window title slot; without it a quoted URL
        // is swallowed as the title.
        c.args(["/C", "start", ""]).arg(url);
        out.push(c);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_candidate_list_is_never_empty_on_this_platform() {
        // $BROWSER may or may not be set; the platform opener is always there.
        assert!(!candidates("https://example.test").is_empty());
    }
}
