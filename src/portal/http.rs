//! A minimal HTTP(S) client that delegates the transport to the system `curl`
//! binary.
//!
//! Why not a crate: this repo treats its dependency count as a contract
//! (`make embed` asserts the embedding build has exactly one third-party
//! crate), and the project's standing answer to "we need X" has been to
//! hand-roll it (PNG, deflate, base64, JSON). TLS is the one thing that must
//! **never** be hand-rolled, so the transport is borrowed from the operating
//! system instead: `curl` ships on effectively every Linux and macOS machine
//! and on Windows 10+. If it is absent, the error says so by name.
//!
//! Why `--config -`: the bearer token and the device code are credentials, and
//! anything on argv is world-readable in `ps` on a shared machine. Every
//! request is therefore described in curl's config-file syntax and handed over
//! on **stdin**; argv carries only `--config -`.

use super::PortalError;
use std::io::Write;
use std::process::{Command, Stdio};

/// One HTTP response: the status line's code and the body.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    /// True for any 2xx.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Hard time limits on every request. An external request with no timeout can
/// park the caller forever; nothing here is allowed to.
const CONNECT_TIMEOUT_S: u32 = 5;
const TOTAL_TIMEOUT_S: u32 = 15;

/// Escape a value for curl's config-file quoting: inside double quotes curl
/// understands `\\` and `\"` (plus a few whitespace escapes we never emit).
fn config_quote(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            // A control character in a URL, token or compact JSON body is
            // never legitimate; refuse it structurally by dropping to space
            // rather than emitting something curl would mis-parse.
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build the curl config for one request — everything that must NOT be on
/// argv lives in this string, which travels on stdin. Separate from
/// [`request`] so a test can pin that property.
fn build_config(
    method: &str,
    url: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> String {
    let mut cfg = String::new();
    cfg.push_str(&format!("url = {}\n", config_quote(url)));
    cfg.push_str(&format!("request = {}\n", config_quote(method)));
    if let Some(t) = token {
        cfg.push_str(&format!(
            "header = {}\n",
            config_quote(&format!("Authorization: Bearer {t}"))
        ));
    }
    if let Some(b) = body {
        cfg.push_str("header = \"Content-Type: application/json\"\n");
        cfg.push_str(&format!("data = {}\n", config_quote(b)));
    }
    cfg.push_str("silent\nshow-error\n");
    cfg.push_str(&format!("connect-timeout = {CONNECT_TIMEOUT_S}\n"));
    cfg.push_str(&format!("max-time = {TOTAL_TIMEOUT_S}\n"));
    // Appended after the body: split on the last newline to recover the code.
    cfg.push_str("write-out = \"\\n%{http_code}\"\n");
    cfg
}

/// Perform one request. `token` becomes an `Authorization: Bearer` header;
/// `body` is sent as `application/json` and forces POST.
pub fn request(
    method: &str,
    url: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> Result<Response, PortalError> {
    let cfg = build_config(method, url, token, body);
    let mut child = Command::new("curl")
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            PortalError::Http(if e.kind() == std::io::ErrorKind::NotFound {
                "the `curl` binary is not installed — the portal client uses it \
                 for HTTPS; install curl and retry"
                    .into()
            } else {
                format!("could not run curl: {e}")
            })
        })?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(cfg.as_bytes())
        .map_err(|e| PortalError::Http(format!("could not hand curl its config: {e}")))?;
    let out = child
        .wait_with_output()
        .map_err(|e| PortalError::Http(format!("curl did not finish: {e}")))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(PortalError::Http(format!(
            "curl failed ({}): {}",
            out.status,
            err.trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (body, code) = stdout
        .rsplit_once('\n')
        .ok_or_else(|| PortalError::Http("curl produced no status code".into()))?;
    let status: u16 = code.trim().parse().map_err(|_| {
        PortalError::Http(format!("curl status line unreadable: '{code}'"))
    })?;
    if status == 0 {
        // write-out prints 000 when no HTTP exchange happened at all.
        return Err(PortalError::Http("no HTTP response from the portal".into()));
    }
    Ok(Response {
        status,
        body: body.to_string(),
    })
}

/// `GET url` with an optional bearer token.
pub fn get(url: &str, token: Option<&str>) -> Result<Response, PortalError> {
    request("GET", url, token, None)
}

/// `POST url` with a JSON body and an optional bearer token.
pub fn post_json(
    url: &str,
    token: Option<&str>,
    body: &str,
) -> Result<Response, PortalError> {
    request("POST", url, token, Some(body))
}

/// Is curl available at all? Used by the test harness to skip rather than
/// fail on a machine without it, and by `terminal login` to fail up front
/// with the real reason instead of mid-flow.
pub fn curl_available() -> bool {
    Command::new("curl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_what_curl_would_misparse() {
        assert_eq!(config_quote(r#"a"b\c"#), r#""a\"b\\c""#);
        // A control character cannot survive into the config stream.
        assert_eq!(config_quote("a\nb"), "\"a b\"");
    }

    #[test]
    fn credentials_travel_in_the_stdin_config_with_timeouts_set() {
        // The whole argv is `curl --config -`; the token and body live in the
        // config stream, and every request carries both time limits.
        let cfg = build_config(
            "POST",
            "http://127.0.0.1:1/x",
            Some("SECRET-TOKEN"),
            Some(r#"{"device_code":"SECRET-CODE"}"#),
        );
        assert!(cfg.contains("Authorization: Bearer SECRET-TOKEN"));
        assert!(cfg.contains("SECRET-CODE"));
        assert!(cfg.contains("connect-timeout"));
        assert!(cfg.contains("max-time"));
        assert!(cfg.contains("write-out"));
    }
}
