//! Sign-in: the RFC 8628 device-code flow and the on-disk token.
//!
//! The token lives at `${XDG_STATE_HOME:-~/.local/state}/terminalgb/portal-token`
//! (through [`app_dir_in`]'s pre-rename fallback, like every other per-user
//! file), mode `0600`, written atomically. It records **which portal it belongs
//! to**: the same fingerprint idea as the remembered volume's `from_config` —
//! if the configured portal URL changes, the stored token is refused rather
//! than sent to a server it was never issued for.
//!
//! [`app_dir_in`]: crate::screen::config::app_dir_in

use super::{http, PortalError};
use std::path::PathBuf;
use terminalgb_plugin_agent::json::{self, Writer};

/// A stored sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The portal base URL this token was issued by.
    pub portal: String,
    pub access: String,
    pub refresh: Option<String>,
    /// The signed-in account name, when the portal reported one.
    pub user: Option<String>,
}

/// What the store holds for a given portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    /// Never signed in (or signed out).
    None,
    /// A token exists but belongs to a different portal URL — treated as
    /// signed out, and the caller should say why.
    Mismatch {
        stored_portal: String,
    },
    Token(Token),
}

/// Where the token lives. `None` when no state directory can be resolved
/// (no `$HOME` — the embedding-adjacent case), in which case sign-in cannot
/// be persisted and says so.
pub fn token_path() -> Option<PathBuf> {
    crate::screen::config::state_dir().map(|d| d.join("portal-token"))
}

/// Read the store and check it against the configured portal.
pub fn load(portal: &str) -> Result<Stored, PortalError> {
    let Some(path) = token_path() else {
        return Ok(Stored::None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Stored::None),
        Err(e) => {
            return Err(PortalError::Store(format!(
                "cannot read {}: {e}",
                path.display()
            )))
        }
    };
    Ok(parse_store(&text, portal))
}

/// The pure half of [`load`]: read the key = value store and judge it against
/// the configured portal.
fn parse_store(text: &str, portal: &str) -> Stored {
    let mut tok = Token {
        portal: String::new(),
        access: String::new(),
        refresh: None,
        user: None,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            match k {
                "portal" => tok.portal = v.to_string(),
                "access" => tok.access = v.to_string(),
                "refresh" if !v.is_empty() => tok.refresh = Some(v.to_string()),
                "user" if !v.is_empty() => tok.user = Some(v.to_string()),
                _ => {}
            }
        }
    }
    if tok.access.is_empty() {
        return Stored::None;
    }
    if tok.portal != portal {
        return Stored::Mismatch {
            stored_portal: tok.portal,
        };
    }
    Stored::Token(tok)
}

/// Persist a sign-in: atomic (tmp → rename, the battery's rule — a torn
/// credential file is worse than none) and private (`0600` from creation,
/// never a window at default permissions).
pub fn save(token: &Token) -> Result<PathBuf, PortalError> {
    let path = token_path().ok_or_else(|| {
        PortalError::Store(
            "no state directory (is $HOME set?) — cannot remember the sign-in".into(),
        )
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            PortalError::Store(format!("cannot create {}: {e}", dir.display()))
        })?;
    }
    let mut text = String::from(
        "# TerminalGB portal sign-in. Private — do not commit or share this file.\n",
    );
    text.push_str(&format!("portal = {}\n", token.portal));
    text.push_str(&format!("access = {}\n", token.access));
    if let Some(r) = &token.refresh {
        text.push_str(&format!("refresh = {r}\n"));
    }
    if let Some(u) = &token.user {
        text.push_str(&format!("user = {u}\n"));
    }
    let tmp = path.with_extension("tmp");
    write_private(&tmp, &text).map_err(|e| {
        PortalError::Store(format!("cannot write {}: {e}", tmp.display()))
    })?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| PortalError::Store(format!("cannot move token into place: {e}")))?;
    Ok(path)
}

/// Sign out: forget the stored token. Missing is fine — signing out twice is
/// not an error.
pub fn forget() -> Result<(), PortalError> {
    let Some(path) = token_path() else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PortalError::Store(format!(
            "cannot remove {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    // Windows has no mode bits; the state directory is already per-user.
    std::fs::write(path, text)
}

// --- The device-code flow ---------------------------------------------------

/// What `POST /api/device/code` hands back: the code the user types and where
/// to type it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// RFC 8628's optional pre-filled variant — when present this is what the
    /// browser is pointed at, so the user only has to approve.
    pub verification_uri_complete: Option<String>,
    /// Seconds between polls (server-controlled).
    pub interval_s: u64,
    /// How long the code lives; the poll loop gives up after this.
    pub expires_in_s: u64,
}

/// The client name this emulator identifies itself as.
const CLIENT_ID: &str = "terminalgb";

fn contract(e: impl std::fmt::Display, what: &str) -> PortalError {
    PortalError::Contract(format!("{what}: {e}"))
}

/// Start a sign-in: ask the portal for a device code.
pub fn device_start(portal: &str) -> Result<DeviceCode, PortalError> {
    let mut w = Writer::new();
    w.begin_obj();
    w.str_field("client_id", CLIENT_ID);
    w.end_obj();
    let resp = http::post_json(&format!("{portal}/api/device/code"), None, &w.finish())?;
    if !resp.ok() {
        return Err(PortalError::Contract(format!(
            "device/code answered {}: {}",
            resp.status,
            resp.body.trim()
        )));
    }
    let j = json::parse(&resp.body)
        .map_err(|e| contract(e, "device/code reply is not JSON"))?;
    let field = |k: &str| -> Result<String, PortalError> {
        j.str_field(k).map(str::to_string).ok_or_else(|| {
            PortalError::Contract(format!("device/code reply is missing '{k}'"))
        })
    };
    Ok(DeviceCode {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        // Accept both RFC spellings rather than making the other crew's
        // choice a breakage.
        verification_uri: j
            .str_field("verification_uri")
            .or_else(|| j.str_field("verification_url"))
            .map(str::to_string)
            .ok_or_else(|| {
                PortalError::Contract(
                    "device/code reply is missing 'verification_uri'".into(),
                )
            })?,
        verification_uri_complete: j
            .str_field("verification_uri_complete")
            .map(str::to_string),
        interval_s: j.u64_field("interval").unwrap_or(5),
        expires_in_s: j.u64_field("expires_in").unwrap_or(900),
    })
}

/// One poll of `POST /api/device/token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user has not approved yet — keep waiting.
    Pending,
    /// The server asked for a longer interval (RFC 8628 `slow_down`).
    SlowDown,
    /// Signed in.
    Token(Token),
    /// The flow is over and did not produce a token (denied, expired).
    Denied(String),
}

pub fn device_poll(portal: &str, device_code: &str) -> Result<Poll, PortalError> {
    let mut w = Writer::new();
    w.begin_obj();
    w.str_field("client_id", CLIENT_ID);
    w.str_field("device_code", device_code);
    w.str_field("grant_type", "urn:ietf:params:oauth:grant-type:device_code");
    w.end_obj();
    let resp = http::post_json(&format!("{portal}/api/device/token"), None, &w.finish())?;
    let j = json::parse(&resp.body)
        .map_err(|e| contract(e, "device/token reply is not JSON"))?;
    if resp.ok() {
        let access = j
            .str_field("access_token")
            .or_else(|| j.str_field("access"))
            .map(str::to_string)
            .ok_or_else(|| {
                PortalError::Contract(
                    "device/token success carried no access token".into(),
                )
            })?;
        return Ok(Poll::Token(Token {
            portal: portal.to_string(),
            access,
            refresh: j
                .str_field("refresh_token")
                .or_else(|| j.str_field("refresh"))
                .map(str::to_string),
            user: j.str_field("user").map(str::to_string),
        }));
    }
    // RFC 8628 reports flow state as an `error` field on a 4xx (the portal
    // sketch used 428; accept any 4xx carrying the field).
    match j.str_field("error") {
        Some("authorization_pending") => Ok(Poll::Pending),
        Some("slow_down") => Ok(Poll::SlowDown),
        Some("access_denied") => Ok(Poll::Denied("the sign-in was declined".into())),
        Some("expired_token") => Ok(Poll::Denied(
            "the code expired before it was entered — start again".into(),
        )),
        Some(other) => Ok(Poll::Denied(format!("the portal refused: {other}"))),
        None => Err(PortalError::Contract(format!(
            "device/token answered {} with no 'error' field: {}",
            resp.status,
            resp.body.trim()
        ))),
    }
}

/// `GET /api/me` with a freshly issued token — learns the account's display
/// name when the token reply carried no `user` field (the live portal's
/// contract does not include one). Fail-soft on purpose: a name is cosmetic,
/// and sign-in must not fail because a lookup did.
pub fn whoami(portal: &str, access: &str) -> Option<String> {
    let resp = http::get(&format!("{portal}/api/me"), Some(access)).ok()?;
    if !resp.ok() {
        return None;
    }
    let j = json::parse(&resp.body).ok()?;
    j.get("user")?.str_field("displayName").map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    // File I/O and permissions are exercised in the integration harness
    // (`testharness/portal.rs`), where $XDG_STATE_HOME can be owned safely.
    // The parsing rules are pinned here on the pure half.

    #[test]
    fn a_store_with_no_access_token_reads_as_signed_out() {
        assert_eq!(parse_store("# empty\nportal = p\n", "p"), Stored::None);
    }

    #[test]
    fn a_token_for_a_different_portal_is_refused_not_sent() {
        // The volume file's `from_config` rule, applied to a credential:
        // the fingerprint decides, so a token can never be sent to a server
        // it was not issued by.
        let text = "portal = https://old.example\naccess = abc\n";
        assert_eq!(
            parse_store(text, "https://new.example"),
            Stored::Mismatch {
                stored_portal: "https://old.example".into()
            }
        );
    }

    #[test]
    fn a_matching_store_round_trips_every_field() {
        let text = "# comment\nportal = https://p.example\naccess = A\nrefresh = R\nuser = aaron\n";
        match parse_store(text, "https://p.example") {
            Stored::Token(t) => {
                assert_eq!(t.access, "A");
                assert_eq!(t.refresh.as_deref(), Some("R"));
                assert_eq!(t.user.as_deref(), Some("aaron"));
            }
            other => panic!("expected a token, got {other:?}"),
        }
    }
}
