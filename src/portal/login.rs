//! The two front ends of the device-code sign-in.
//!
//! [`run_cli`] is `terminal login`: blocking, printing each step. It always
//! prints the URL and code — the browser opening is a convenience on top, so
//! a headless or SSH session signs in the same way, just by hand.
//!
//! [`LoginFlow`] is the same flow for the TUI, on its own thread: the event
//! loop polls [`LoginFlow::status`] and never blocks — the same rule as every
//! other slow thing near the frame loop (a network wait inside a frame would
//! be the audio player's blocking hand-off all over again).

use super::{auth, browser, PortalError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a sign-in attempt is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStatus {
    /// Contacting the portal for a code.
    Starting,
    /// Waiting for the user to approve in the browser.
    Waiting {
        user_code: String,
        verification_uri: String,
        /// Whether a browser was launched — decides the wording, nothing else.
        opened_browser: bool,
    },
    /// Signed in (and the token is stored).
    Done {
        user: Option<String>,
    },
    Failed(String),
}

impl LoginStatus {
    pub fn is_over(&self) -> bool {
        matches!(self, LoginStatus::Done { .. } | LoginStatus::Failed(_))
    }
}

/// A sign-in running on its own thread.
pub struct LoginFlow {
    status: Arc<Mutex<LoginStatus>>,
}

impl LoginFlow {
    /// Start signing in to `portal`. Returns immediately; poll [`status`].
    ///
    /// [`status`]: LoginFlow::status
    pub fn start(portal: String) -> LoginFlow {
        let status = Arc::new(Mutex::new(LoginStatus::Starting));
        let shared = status.clone();
        std::thread::spawn(move || {
            let outcome = run_flow(&portal, |st| {
                *shared.lock().unwrap() = st;
            });
            if let Err(e) = outcome {
                *shared.lock().unwrap() = LoginStatus::Failed(e.to_string());
            }
        });
        LoginFlow { status }
    }

    pub fn status(&self) -> LoginStatus {
        self.status.lock().unwrap().clone()
    }
}

/// The flow itself, reporting transitions through `report`. On success the
/// token is already stored when `Done` is reported.
fn run_flow(portal: &str, report: impl Fn(LoginStatus)) -> Result<(), PortalError> {
    let code = auth::device_start(portal)?;
    // Point the browser at the pre-filled variant when the portal offers one,
    // so the user only has to approve; the plain URI + code path is always
    // reported alongside for the fallback wording.
    let open_target = code
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&code.verification_uri);
    let opened = browser::open(open_target);
    report(LoginStatus::Waiting {
        user_code: code.user_code.clone(),
        verification_uri: code.verification_uri.clone(),
        opened_browser: opened,
    });

    let deadline = Instant::now() + Duration::from_secs(code.expires_in_s.max(5));
    let mut interval = code.interval_s;
    loop {
        if Instant::now() >= deadline {
            return Err(PortalError::Auth(
                "the sign-in code expired before it was approved — start again".into(),
            ));
        }
        std::thread::sleep(Duration::from_secs(interval.min(60)));
        match auth::device_poll(portal, &code.device_code)? {
            auth::Poll::Pending => {}
            auth::Poll::SlowDown => interval += 5,
            auth::Poll::Denied(why) => return Err(PortalError::Auth(why)),
            auth::Poll::Token(mut token) => {
                if token.user.is_none() {
                    token.user = auth::whoami(portal, &token.access);
                }
                auth::save(&token)?;
                report(LoginStatus::Done {
                    user: token.user.clone(),
                });
                return Ok(());
            }
        }
    }
}

/// `terminal login`, blocking. Returns a process exit code.
pub fn run_cli(portal: Option<&str>) -> i32 {
    let Some(portal) = super::portal_url(portal) else {
        eprintln!(
            "terminal login: no portal configured.\n\
             Set `portal = <url>` in config.toml (or GB_PORTAL=<url>) first."
        );
        return 2;
    };
    if !super::http::curl_available() {
        eprintln!(
            "terminal login: the `curl` binary is not installed — the portal \
             client uses it for HTTPS. Install curl and retry."
        );
        return 2;
    }
    println!("Signing in to {portal} …");
    let outcome = run_flow(&portal, |st| match st {
        LoginStatus::Waiting {
            user_code,
            verification_uri,
            opened_browser,
        } => {
            if opened_browser {
                println!("A browser window should have opened.");
                println!("If it did not: go to  {verification_uri}");
            } else {
                println!("Go to  {verification_uri}");
            }
            println!("and enter the code  {user_code}");
            println!("Waiting for approval …");
        }
        LoginStatus::Done { user } => match user {
            Some(u) => println!("Signed in as {u}."),
            None => println!("Signed in."),
        },
        _ => {}
    });
    match outcome {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("terminal login: {e}");
            1
        }
    }
}

/// `terminal logout`: forget the stored token. Local only — the portal is not
/// told, which keeps this working offline; a stolen token is revoked on the
/// portal's account page.
pub fn run_logout_cli() -> i32 {
    match auth::forget() {
        Ok(()) => {
            println!("Signed out (the stored token was removed).");
            0
        }
        Err(e) => {
            eprintln!("terminal logout: {e}");
            1
        }
    }
}
