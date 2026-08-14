//! The portal client, end to end, against a local stub server.
//!
//! The real portal is a separate repository being built in parallel; what the
//! two sides share is an HTTP contract, so this harness **is** the contract
//! from the emulator's side: a `TcpListener` on 127.0.0.1 speaking exactly the
//! request/response bodies `docs/portal.md` documents. Everything the client
//! does — curl transport, device-code sign-in on its own thread, token
//! storage and permissions, the portal-URL fingerprint, trade fetch, spec
//! validation, completion confirm — runs here for real; the only thing this
//! cannot prove is that the *actual* portal serves the same shapes.
//!
//! Needs the system `curl` binary (the transport). Skips with exit 0 and a
//! message when it is absent, like the ROM-gated harnesses do.
//!
//! Run: `cargo test --release --test portal`

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use terminalgb::portal::{
    auth, client::Portal, http, login, source::PortalSource, PortalError,
};

// --- the stub -------------------------------------------------------------

struct Stub {
    /// How many device/token polls answer `authorization_pending` first.
    pending_polls: u32,
    token_polls: AtomicU32,
    /// Body served by GET /api/trades/mine (when authorised).
    trades_body: Mutex<String>,
    /// Every confirmed trade: (path, bearer token presented).
    done_hits: Mutex<Vec<(String, Option<String>)>>,
}

const ACCESS: &str = "tok-abc";

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes());
}

fn handle(mut stream: TcpStream, stub: &Stub) {
    // Read the request: header block, then Content-Length of body.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return,
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 64 * 1024 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("").to_string();
    let mut bearer = None;
    let mut content_length = 0usize;
    for l in lines {
        let lower = l.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("authorization: bearer ") {
            bearer = Some(v.trim().to_string());
        }
        if let Some(v) = lower.strip_prefix("content-length: ") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    while buf.len() < header_end + content_length {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
    let mut parts = request.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));

    match (method, path) {
        ("POST", "/api/device/code") => {
            // Pin the request shape: the client must identify itself.
            if !body.contains("\"client_id\":\"terminalgb\"") {
                return respond(
                    &mut stream,
                    "400 Bad Request",
                    r#"{"error":"no client_id"}"#,
                );
            }
            let addr = stream
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let reply = format!(
                r#"{{"device_code":"dev-1","user_code":"WXYZ-1234",
                    "verification_uri":"http://{addr}/link","interval":1,"expires_in":60}}"#
            );
            respond(&mut stream, "200 OK", &reply);
        }
        ("POST", "/api/device/token") => {
            // Pin the poll body: the device code must come back.
            if !body.contains("\"device_code\":\"dev-1\"") {
                return respond(
                    &mut stream,
                    "400 Bad Request",
                    r#"{"error":"access_denied"}"#,
                );
            }
            let n = stub.token_polls.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= stub.pending_polls {
                respond(
                    &mut stream,
                    "428 Precondition Required",
                    r#"{"error":"authorization_pending"}"#,
                );
            } else {
                respond(
                    &mut stream,
                    "200 OK",
                    &format!(
                        r#"{{"access_token":"{ACCESS}","refresh_token":"ref-1","user":"aaron"}}"#
                    ),
                );
            }
        }
        ("GET", "/api/trades/mine") => {
            if bearer.as_deref() != Some(ACCESS) {
                return respond(
                    &mut stream,
                    "401 Unauthorized",
                    r#"{"error":"unauthorized"}"#,
                );
            }
            let body = stub.trades_body.lock().unwrap().clone();
            respond(&mut stream, "200 OK", &body);
        }
        ("POST", p) if p.starts_with("/api/trades/") && p.ends_with("/done") => {
            if bearer.as_deref() != Some(ACCESS) {
                return respond(
                    &mut stream,
                    "401 Unauthorized",
                    r#"{"error":"unauthorized"}"#,
                );
            }
            stub.done_hits.lock().unwrap().push((p.to_string(), bearer));
            respond(&mut stream, "200 OK", "{}");
        }
        _ => respond(&mut stream, "404 Not Found", r#"{"error":"not found"}"#),
    }
}

fn start_stub(stub: Arc<Stub>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let stub = stub.clone();
            std::thread::spawn(move || handle(stream, &stub));
        }
    });
    format!("http://{addr}")
}

// --- the checks -----------------------------------------------------------

static FAILURES: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool, detail: &str) {
    if ok {
        println!("PASS  {name}");
    } else {
        println!("FAIL  {name} — {detail}");
        FAILURES.fetch_add(1, Ordering::SeqCst);
    }
}

const ONE_TRADE: &str = r#"{"trades":[{
    "id":71,"friend":"ben","give":"CHARIZARD lv36","trainer":"BEN",
    "receive":{"species":"Bulbasaur","level":14,"nickname":"BULBA",
               "ot_name":"BEN","ot_id":12345,
               "moves":["Tackle","Growl"],"dvs":[10,8,12,15]}}]}"#;

fn main() {
    if !http::curl_available() {
        println!("portal harness: `curl` is not installed on this machine — skipping");
        return;
    }

    // Own the process environment before any thread starts. The token store
    // goes to a scratch state dir so this harness can never touch the real
    // one; the "browser" is coreutils' `true`, so the open path runs for
    // real without a window appearing on anyone's desktop.
    let state = std::env::temp_dir()
        .join(format!("terminalgb-portal-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).expect("scratch state dir");
    std::env::set_var("XDG_STATE_HOME", &state);
    std::env::set_var("BROWSER", "true");
    std::env::remove_var("GB_PORTAL");
    std::env::remove_var("GB_PORTAL_NO_BROWSER");

    let stub = Arc::new(Stub {
        pending_polls: 2,
        token_polls: AtomicU32::new(0),
        trades_body: Mutex::new(ONE_TRADE.to_string()),
        done_hits: Mutex::new(Vec::new()),
    });
    let base = start_stub(stub.clone());

    // [1] The whole sign-in, through the TUI's thread-backed flow: request a
    // code, "open" the browser, poll through two pendings, store the token.
    let flow = login::LoginFlow::start(base.clone());
    let mut saw_waiting_with_browser = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    let final_status = loop {
        let st = flow.status();
        if let login::LoginStatus::Waiting {
            opened_browser,
            user_code,
            ..
        } = &st
        {
            if *opened_browser && user_code == "WXYZ-1234" {
                saw_waiting_with_browser = true;
            }
        }
        if st.is_over() {
            break st;
        }
        if Instant::now() > deadline {
            break st;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    check(
        "the device-code flow signs in",
        matches!(&final_status, login::LoginStatus::Done { user: Some(u) } if u == "aaron"),
        &format!("{final_status:?}"),
    );
    check(
        "the code and a launched browser were reported while waiting",
        saw_waiting_with_browser,
        "never saw Waiting{opened_browser:true, code WXYZ-1234}",
    );
    check(
        "the server was polled through authorization_pending",
        stub.token_polls.load(Ordering::SeqCst) >= 3,
        &format!("{} polls", stub.token_polls.load(Ordering::SeqCst)),
    );

    // [2] The stored token: private, and fingerprinted to this portal.
    let path = auth::token_path().expect("token path");
    check(
        "the token file exists",
        path.is_file(),
        &path.display().to_string(),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        check(
            "the token file is private (0600)",
            mode == 0o600,
            &format!("mode {mode:o}"),
        );
    }
    match auth::load(&base) {
        Ok(auth::Stored::Token(t)) => {
            check(
                "the stored token round-trips",
                t.access == ACCESS && t.user.as_deref() == Some("aaron"),
                &format!("{t:?}"),
            );
        }
        other => check("the stored token round-trips", false, &format!("{other:?}")),
    }
    check(
        "a token is never sent to a different portal",
        matches!(
            auth::load("http://127.0.0.1:1"),
            Ok(auth::Stored::Mismatch { .. })
        ),
        "expected Stored::Mismatch",
    );

    // [3] Fetching the pending trade.
    let portal = Portal::connect(&base).expect("connect");
    check(
        "the signed-in user is known",
        portal.user() == Some("aaron"),
        &format!("{:?}", portal.user()),
    );
    match PortalSource::fetch(&portal) {
        Ok(Some(src)) => {
            check(
                "the pending trade arms a source",
                src.trade_id == "71"
                    && src.friend == "ben"
                    && src.give == "CHARIZARD lv36"
                    && src.incoming() == "Bulbasaur",
                &format!(
                    "id={} friend={} give={} incoming={}",
                    src.trade_id,
                    src.friend,
                    src.give,
                    src.incoming()
                ),
            );
            let d = src.describe_line();
            check(
                "the source says where the Pokémon came from",
                d.contains("#71") && d.contains("ben"),
                &d,
            );
        }
        other => check(
            "the pending trade arms a source",
            false,
            &format!("{:?}", other.map(|o| o.is_some())),
        ),
    }

    // [4] No pending trade is a normal answer, not an error.
    *stub.trades_body.lock().unwrap() = r#"{"trades":[]}"#.to_string();
    check(
        "an empty trade list is Ok(None)",
        matches!(PortalSource::fetch(&portal), Ok(None)),
        "expected Ok(None)",
    );

    // [5] A malformed Pokémon is refused loudly, naming the field — never
    // armed, never silently dropped.
    *stub.trades_body.lock().unwrap() = r#"{"trades":[{
        "id":9,"friend":"mallory",
        "receive":{"species":"Mew","level":5,"ot_id":1,"dvs":[16,0,0,0]}}]}"#
        .to_string();
    match PortalSource::fetch(&portal) {
        Err(PortalError::Contract(e)) => check(
            "an out-of-range DV is refused by name",
            e.contains("DV") && e.contains("9"),
            &e,
        ),
        other => check(
            "an out-of-range DV is refused by name",
            false,
            &format!("{:?}", other.map(|o| o.is_some())),
        ),
    }

    // [5b] The live portal's contract shape (`api-contract.md`): camelCase OT
    // fields, a dvs *object*, the trade relative to the viewer (`youReceive`,
    // `youGive` as a full spec, `partner.displayName`). Verified against the
    // real portal 2026-08-14; this pins what that run proved.
    *stub.trades_body.lock().unwrap() = r#"{"trades":[{
        "id":10,"status":"accepted","role":"acceptor",
        "partner":{"displayName":"trainer_red","friendCode":"2NJW4P"},
        "youGive":{"species":"Squirtle","level":11,"nickname":"SQUIRT",
                   "otName":"BLUE","otId":24601,"moves":["Tackle"],
                   "dvs":{"attack":12,"defense":10,"speed":13,"special":9}},
        "youReceive":{"species":"Charmander","level":12,"nickname":"CHARRY",
                      "otName":"RED","otId":31337,"moves":["Scratch"],
                      "dvs":{"attack":11,"defense":9,"speed":14,"special":7}},
        "youDone":false,"partnerDone":false}]}"#
        .to_string();
    match PortalSource::fetch(&portal) {
        Ok(Some(src)) => check(
            "the portal's contract shape arms a source",
            src.trade_id == "10"
                && src.friend == "trainer_red"
                && src.give == "SQUIRT lv11"
                && src.incoming() == "Charmander",
            &format!(
                "id={} friend={} give={} incoming={}",
                src.trade_id,
                src.friend,
                src.give,
                src.incoming()
            ),
        ),
        other => check(
            "the portal's contract shape arms a source",
            false,
            &format!("{:?}", other.map(|o| o.is_some())),
        ),
    }

    // [5c] trades/mine includes completed and already-confirmed trades;
    // arming either would replay a finished trade, so both are skipped.
    *stub.trades_body.lock().unwrap() = r#"{"trades":[
        {"id":8,"status":"completed",
         "partner":{"displayName":"x"},
         "youReceive":{"species":"Mew","level":5,"otId":1,
                       "dvs":{"attack":0,"defense":0,"speed":0,"special":0}}},
        {"id":9,"status":"accepted","youDone":true,
         "partner":{"displayName":"x"},
         "youReceive":{"species":"Mew","level":5,"otId":1,
                       "dvs":{"attack":0,"defense":0,"speed":0,"special":0}}}]}"#
        .to_string();
    check(
        "completed and already-confirmed trades are never re-armed",
        matches!(PortalSource::fetch(&portal), Ok(None)),
        "expected Ok(None)",
    );

    // [6] Confirming completion reaches the portal, authenticated.
    portal.trade_done("71").expect("trade_done");
    {
        let hits = stub.done_hits.lock().unwrap();
        check(
            "trade_done POSTs to /api/trades/71/done with the bearer token",
            hits.len() == 1
                && hits[0].0 == "/api/trades/71/done"
                && hits[0].1.as_deref() == Some(ACCESS),
            &format!("{hits:?}"),
        );
    }

    // [7] A dead portal is a loud transport error, not a hang: the client's
    // own timeouts bound this (nothing is listening on this port).
    let t = Instant::now();
    let dead = http::get("http://127.0.0.1:9/api/x", None);
    check(
        "an unreachable portal fails fast and loudly",
        dead.is_err() && t.elapsed() < Duration::from_secs(10),
        &format!(
            "{:?} after {:?}",
            dead.as_ref().map(|r| r.status),
            t.elapsed()
        ),
    );

    // [8] Signing out removes the token; doing it twice is fine.
    auth::forget().expect("forget");
    auth::forget().expect("forget twice");
    check(
        "logout forgets the token",
        matches!(auth::load(&base), Ok(auth::Stored::None)),
        "expected Stored::None",
    );
    check(
        "a signed-out connect says how to sign in",
        matches!(Portal::connect(&base), Err(PortalError::Auth(ref e)) if e.contains("login")),
        "expected an Auth error naming `terminal login`",
    );

    let _ = std::fs::remove_dir_all(&state);
    let failures = FAILURES.load(Ordering::SeqCst);
    if failures > 0 {
        println!("\n{failures} check(s) FAILED");
        std::process::exit(1);
    }
    println!("\nAll portal checks passed.");
}
