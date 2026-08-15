//! Does the terminal frontend still run in a terminal that has **no kitty
//! keyboard protocol**?
//!
//! `8357546` made the answer "no": `screen::tui::run` returned an error the
//! moment `supports_keyboard_enhancement()` said `Ok(false)`, so on GNOME
//! Terminal / Ptyxis (VTE), Konsole and xterm the emulator printed one line and
//! exited having drawn nothing. Nothing in the suite noticed, because every
//! other harness builds a `Gameboy` directly and never enters the frontend at
//! all — the whole entry path (capability handshakes, refusals, input mapping)
//! was untested.
//!
//! So this harness **is** the terminal. It allocates a pty, becomes the thing
//! on the other end of it, and answers the frontend's capability questions the
//! way a chosen terminal would:
//!
//! * `CSI c`  (primary device attributes) — answered, as every terminal does.
//! * `CSI 6 n` (cursor position) — answered, as every terminal does.
//! * `CSI ? u` (kitty keyboard) — **answered only in the `kitty` scenario.**
//!   Silence is precisely what a VTE terminal does, and is the path under test.
//! * kitty graphics `APC G` — never answered; the picture must fall back too.
//!
//! Two runs, same binary, differing in that one reply, and the frontend must
//! report `basic` in one and `kitty` in the other. That is what makes this a
//! test of the fallback rather than an assumption about it.
//!
//! No window is opened and no synthetic input is sent to the desktop: the child
//! gets its own session (`setsid` + `TIOCSCTTY`) so even its `/dev/tty` probes
//! land on our pty. It runs anywhere `cargo test` runs, CI included.
//!
//! Run it with `cargo test --release --test basicterm`.

use std::io::{Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Set in the child so one binary can be both the terminal and the app.
const CHILD_ENV: &str = "GB_BASICTERM_CHILD";
const ROM_ENV: &str = "GB_BASICTERM_ROM";
/// Window size the fake terminal presents. Wide enough for the sidebar panels
/// the assertions read.
const COLS: u16 = 120;
const ROWS: u16 = 44;

fn main() {
    if std::env::var_os(CHILD_ENV).is_some() {
        child_main();
    }
    parent_main();
}

// ---------------------------------------------------------------------------
// The child: the real frontend, on the far end of our pty.
// ---------------------------------------------------------------------------

fn child_main() -> ! {
    let rom = std::env::var(ROM_ENV).expect("ROM path");
    // Exactly what `examples/terminal` does, so the path under test is the
    // product's own and not a reimplementation of it. `run_terminal` prints a
    // setup error and exits **1** itself — which is precisely how the
    // regression manifested, so the parent can watch for it.
    terminalgb::gameboy::run_terminal(Some(std::path::PathBuf::from(rom)));
    std::process::exit(0)
}

// ---------------------------------------------------------------------------
// The parent: a fake terminal.
// ---------------------------------------------------------------------------

/// Which capabilities our fake terminal admits to.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Fake {
    /// A VTE-family terminal: answers DA and CPR, ignores the kitty keyboard
    /// query. This is the captain's GNOME Terminal / Ptyxis.
    Basic,
    /// Answers the kitty keyboard query too.
    Kitty,
}

impl Fake {
    fn name(self) -> &'static str {
        match self {
            Fake::Basic => "basic",
            Fake::Kitty => "kitty",
        }
    }
}

fn parent_main() -> ! {
    let rom = stage_rom();
    let mut failures = Vec::new();

    for fake in [Fake::Basic, Fake::Kitty] {
        println!("\n=== fake terminal: {} ===", fake.name());
        if let Err(e) = run_scenario(fake, &rom) {
            println!("  FAIL: {e}");
            failures.push(format!("{}: {e}", fake.name()));
        }
    }

    println!();
    if failures.is_empty() {
        println!("basicterm: all checks passed");
        std::process::exit(0);
    }
    for f in &failures {
        println!("basicterm FAILED — {f}");
    }
    std::process::exit(1);
}

fn run_scenario(fake: Fake, rom: &std::path::Path) -> Result<(), String> {
    let (master, slave) = open_pty(COLS, ROWS)?;

    // Isolate the run from the developer's own config, recents, saved volume
    // and screenshot directory — a test must not read or write those.
    let home =
        std::env::temp_dir().join(format!("gameboy-basicterm/{}", std::process::id()));
    let _ = std::fs::create_dir_all(&home);

    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let mut child = unsafe {
        Command::new(exe)
            .env(CHILD_ENV, "1")
            .env(ROM_ENV, rom)
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_STATE_HOME", &home)
            .env("XDG_PICTURES_DIR", &home)
            // The transports are not what is under test, and the shm handshake
            // costs seconds of probe timeout against a terminal that will never
            // answer it.
            .env("GB_TERM_KITTY", "noshm")
            .env_remove("GB_TELEMETRY")
            .stdin(Stdio::from_raw_fd_dup(slave)?)
            .stdout(Stdio::from_raw_fd_dup(slave)?)
            .stderr(Stdio::from_raw_fd_dup(slave)?)
            // Its own session, with the pty as controlling terminal, so nothing
            // it does can reach the terminal this test was launched from.
            .pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            })
            .spawn()
            .map_err(|e| format!("spawn: {e}"))?
    };
    unsafe { libc::close(slave) };

    let screen = Arc::new(Mutex::new(String::new()));
    let done = Arc::new(AtomicBool::new(false));
    let reader = spawn_fake_terminal(master, fake, screen.clone(), done.clone());

    let result = drive(fake, master, &screen, &mut child);

    // Tear down whatever state the assertions left behind.
    done.store(true, Ordering::Relaxed);
    let _ = child.kill();
    let status = child.wait().ok();
    unsafe { libc::close(master) };
    let _ = reader.join();

    if result.is_ok() {
        println!("  child exited: {status:?}");
    } else if let Ok(s) = screen.lock() {
        // On failure the transcript is the evidence; print the readable part.
        println!("  --- last 400 chars of terminal text ---");
        let text = visible(&s);
        let tail: String = text
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        println!("  {tail}");
    }
    result
}

/// The assertions, in the order a user would hit them.
fn drive(
    fake: Fake,
    master: RawFd,
    screen: &Arc<Mutex<String>>,
    child: &mut std::process::Child,
) -> Result<(), String> {
    // 1. It must not refuse. The regression exited 1 within milliseconds with
    //    "no support for the kitty keyboard protocol" and drew nothing.
    wait_for(screen, "Status", Duration::from_secs(20))
        .map_err(|_| "the frontend never drew a Status panel".to_string())?;
    if let Ok(Some(st)) = child.try_wait() {
        return Err(format!(
            "the frontend exited early ({st}) instead of running"
        ));
    }
    println!("  drew the UI");

    // 2. It must have taken the mode the terminal actually offered, and say so
    //    where a user can see it.
    let want = match fake {
        Fake::Basic => "basic",
        Fake::Kitty => "kitty ·",
    };
    let text = visible(&screen.lock().unwrap());
    let row = status_value(&text, "Input")
        .ok_or_else(|| "no `Input` row in the status panel".to_string())?;
    if !row.contains(want) {
        return Err(format!(
            "Input row is {row:?}, expected it to contain {want:?}"
        ));
    }
    println!("  status panel reports: Input {row}");

    // 3. The degradation must be *stated*, once, and only when it applies.
    let said = text.contains("does not report key");
    match (fake, said) {
        (Fake::Basic, false) => {
            return Err("no notice explaining what is degraded".to_string())
        }
        (Fake::Kitty, true) => {
            return Err("warned about a degradation that did not happen".to_string())
        }
        _ => {}
    }
    println!(
        "  degradation notice: {}",
        if said { "shown" } else { "not needed" }
    );

    // 4. Keys must reach the game. `[` cycles the palette
    //    grayscale -> green -> amber, and the status panel names the live one,
    //    so the effect is visible in the byte stream.
    //
    //    Two presses, so the palette ends on `amber` — a value it cannot have
    //    reached without both landing.
    //
    //    Then **resize the pty** before reading. `ratatui` redraws only the
    //    cells that changed, and it diffs *per cell*: "green" -> "amber" shares
    //    its fourth letter, so the wire carries `amb`, a skip, `r`. Matching
    //    those fragments would be matching an implementation detail of the
    //    diff. A resize clears `ratatui`'s back buffer, so the next frame is a
    //    full repaint and the word is on the wire whole.
    let m0 = mark(screen);
    for _ in 0..2 {
        write_all(master, b"[")?;
        // Clear of the auto-repeat lockout: these are two deliberate presses.
        std::thread::sleep(Duration::from_millis(400));
    }
    repaint(master, screen)?;
    if !since(screen, m0).contains("amber") {
        return Err(format!(
            "keypresses had no effect: the palette never reached amber. Drawn \
             since the first press: {:?}",
            since(screen, m0)
        ));
    }
    println!("  keypresses accepted: palette grayscale -> green -> amber");

    // 5. And the terminal's auto-repeat must not be mistaken for the user
    //    pressing a one-shot control key over and over. Ten `[` one repeat
    //    interval apart is exactly what holding the key down puts on the wire;
    //    unsuppressed they would cycle the palette round and round, and
    //    "amber" -> "grayscale" shares no prefix so a single wrap draws the
    //    whole word. (Kitty mode gets explicit `Repeat` events instead and
    //    keeps its existing behaviour, so the assertion is only made here.)
    if fake == Fake::Basic {
        // **Twelve**, and the count is load-bearing. The first press of a
        // burst is a real press — it follows a pause — so correct behaviour is
        // exactly one step, amber -> grayscale. Twelve unsuppressed steps land
        // back on amber (the cycle is three long), so the two outcomes are
        // distinguishable; ten would leave the palette on grayscale either way
        // and the assertion would pass on the broken code.
        //
        // **Written in one go, and that is what makes this check survive a
        // loaded machine.** It used to be twelve writes with a 30 ms `sleep`
        // between them — but `sleep(30ms)` on a busy box is however long the
        // scheduler feels like, so the harness would itself deliver presses far
        // enough apart to *be* separate presses, and the emulator was then
        // right to act on them. Measured by the captain: 2 failures in 3 runs
        // at load average 8-15, green on a quiet machine. A burst on the wire
        // is what auto-repeat is; how fast this process can pace one is not
        // part of the claim, so it is taken out of the measurement rather than
        // tuned. (The 200 ms timed half of the rule is exercised at unit speed
        // by `auto_repeat_cannot_strobe_a_one_shot_control_key`, which owns its
        // clock.) The emulator's own half of the same problem — its event loop
        // takes one event per iteration, so the lockout measured *its* latency
        // rather than the terminal's — is `App::input_queued_behind`.
        write_all(master, &[b'['; 12])?;
        std::thread::sleep(Duration::from_millis(800));
        let m1 = mark(screen);
        repaint(master, screen)?;
        let after = since(screen, m1);
        if !after.contains("grayscale") || after.contains("amber") {
            return Err(format!(
                "auto-repeat strobed a one-shot control key: 12 repeats moved the \
                 palette more than the one step the leading press earns. Drawn: \
                 {after:?}"
            ));
        }
        println!(
            "  auto-repeat suppressed: 12 repeats moved the palette one step \
             (amber -> grayscale), not twelve"
        );
    }

    // 6. It must also *leave* cleanly — a frontend that cannot be quit in a
    //    basic terminal is barely better than one that will not start.
    write_all(master, b"qy")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(st)) = child.try_wait() {
            if !st.success() {
                return Err(format!("quitting gave {st}"));
            }
            println!("  quit cleanly");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("did not quit on `q` + `y`".to_string())
}

// ---------------------------------------------------------------------------
// The fake terminal itself.
// ---------------------------------------------------------------------------

/// Read everything the frontend writes, and answer the queries a real terminal
/// of this kind would answer — no more.
fn spawn_fake_terminal(
    master: RawFd,
    fake: Fake,
    screen: Arc<Mutex<String>>,
    done: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut file = unsafe { std::fs::File::from_raw_fd(dup(master)) };
        let mut buf = [0u8; 8192];
        let mut pending = Vec::new();
        while !done.load(Ordering::Relaxed) {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Ok(mut s) = screen.lock() {
                        s.push_str(&chunk);
                    }
                    // Queries can straddle a read boundary, and the frontend
                    // sends several in one write (crossterm emits the kitty
                    // keyboard query and the device-attributes query back to
                    // back), so scan a rolling buffer rather than the tail.
                    pending.extend_from_slice(&buf[..n]);
                    let (replies, consumed) = answers(&pending, fake);
                    pending.drain(..consumed);
                    // Keep only enough to reassemble a query split across reads.
                    if pending.len() > 64 {
                        let cut = pending.len() - 64;
                        pending.drain(..cut);
                    }
                    for r in replies {
                        let _ = write_all(master, r.as_bytes());
                    }
                }
                Err(_) => break, // the child closed its end
            }
        }
    })
}

/// What this fake terminal replies to what it has been asked, in the order the
/// questions arrived. Returns the replies and how many bytes were accounted
/// for, so a query is never answered twice.
///
/// Deliberately narrow — three sequences. The one line that separates the two
/// scenarios is the `CSI ? u` arm, and everything else in this harness exists
/// to make that line's effect observable.
fn answers(pending: &[u8], fake: Fake) -> (Vec<String>, usize) {
    // (query, reply). `None` means "seen and deliberately ignored", which is
    // how a terminal without the capability behaves — it must still be
    // *consumed*, or we would keep re-answering it.
    // A fake terminal that stays *silent* where a real one answers is not a
    // faithful stand-in — every unanswered question costs the frontend a
    // multi-second timeout in a blocking startup path, and the first version of
    // this harness drove the emulator at 1 % of real speed for exactly that
    // reason. So answer everything a VTE-family terminal answers, and be silent
    // only where a VTE-family terminal is silent.
    let table: [(&str, Option<&str>); 7] = [
        // Cursor position. Every terminal answers this, and several of the
        // frontend's probes use it as a fence, so silence would hang them.
        ("\x1b[6n", Some("\x1b[1;1R")),
        // Device status: "I am fine."
        ("\x1b[5n", Some("\x1b[0n")),
        // Primary device attributes. crossterm sends this straight after the
        // kitty keyboard query and treats it as "you may stop waiting".
        ("\x1b[0c", Some("\x1b[?62;1;6c")),
        ("\x1b[c", Some("\x1b[?62;1;6c")),
        // Window size in pixels, and cell size in pixels — `ratatui-image` asks
        // both to work out how big an image to build.
        ("\x1b[14t", Some("\x1b[4;704;960t")),
        ("\x1b[16t", Some("\x1b[6;16;8t")),
        // THE query under test.
        (
            "\x1b[?u",
            match fake {
                Fake::Kitty => Some("\x1b[?1u"),
                Fake::Basic => None,
            },
        ),
    ];

    let s = String::from_utf8_lossy(pending).to_string();
    // Collect every hit with its position so replies go out in the order the
    // questions were asked — crossterm cares about that.
    let mut hits: Vec<(usize, usize, Option<&str>)> = Vec::new();
    for (q, r) in table {
        let mut from = 0;
        while let Some(i) = s[from..].find(q) {
            let at = from + i;
            hits.push((at, q.len(), r));
            from = at + q.len();
        }
    }
    hits.sort_by_key(|h| h.0);
    // `CSI [c` is a suffix of `CSI [0c`; keep only the outermost match.
    hits.dedup_by(|b, a| b.0 < a.0 + a.1);

    let consumed = hits.last().map(|h| h.0 + h.1).unwrap_or(0);
    let replies = hits
        .into_iter()
        .filter_map(|(_, _, r)| r.map(str::to_string))
        .collect();
    (replies, consumed)
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// Nudge the window size, which makes `ratatui` throw away its back buffer and
/// repaint every cell — the only way to read a *word* rather than a diff
/// fragment out of the byte stream. Returns once the full frame has arrived.
fn repaint(master: RawFd, screen: &Arc<Mutex<String>>) -> Result<(), String> {
    // Two nudges so the size ends where it started, and so the assertion is
    // never reading a frame drawn at an odd width.
    for cols in [COLS - 1, COLS] {
        set_winsize(master, cols, ROWS)?;
        std::thread::sleep(Duration::from_millis(400));
    }
    // The Controls panel is the bottom of a full frame: once it is back, the
    // repaint is complete.
    wait_for(screen, "Controls", Duration::from_secs(5))
        .map_err(|_| "the frontend stopped drawing after a resize".to_string())
}

fn set_winsize(fd: RawFd, cols: u16, rows: u16) -> Result<(), String> {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: cols * 8,
        ws_ypixel: rows * 16,
    };
    let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
    if rc != 0 {
        return Err(format!("TIOCSWINSZ: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

fn open_pty(cols: u16, rows: u16) -> Result<(RawFd, RawFd), String> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: cols * 8,
        ws_ypixel: rows * 16,
    };
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &ws as *const libc::winsize as *mut libc::winsize,
        )
    };
    if rc != 0 {
        return Err(format!(
            "openpty failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok((master, slave))
}

fn dup(fd: RawFd) -> RawFd {
    let d = unsafe { libc::dup(fd) };
    assert!(d >= 0, "dup failed: {}", std::io::Error::last_os_error());
    d
}

trait FromRawFdDup {
    fn from_raw_fd_dup(fd: RawFd) -> Result<Stdio, String>;
}
impl FromRawFdDup for Stdio {
    fn from_raw_fd_dup(fd: RawFd) -> Result<Stdio, String> {
        Ok(unsafe { Stdio::from_raw_fd(dup(fd)) })
    }
}

fn write_all(fd: RawFd, bytes: &[u8]) -> Result<(), String> {
    let mut f = unsafe { std::fs::File::from_raw_fd(dup(fd)) };
    f.write_all(bytes)
        .and_then(|_| f.flush())
        .map_err(|e| format!("write to pty: {e}"))
}

fn wait_for(
    screen: &Arc<Mutex<String>>,
    needle: &str,
    timeout: Duration,
) -> Result<(), ()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if visible(&screen.lock().unwrap()).contains(needle) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(())
}

/// How much has been received so far — the point after which a keypress's
/// effect must show up.
fn mark(screen: &Arc<Mutex<String>>) -> usize {
    screen.lock().unwrap().len()
}

/// Everything drawn since `mark`, as readable text.
fn since(screen: &Arc<Mutex<String>>, mark: usize) -> String {
    let s = screen.lock().unwrap();
    visible(&s[mark.min(s.len())..])
}

fn status_value(text: &str, key: &str) -> Option<String> {
    let mut last = None;
    for (i, _) in text.match_indices(key) {
        let rest = &text[i + key.len()..];
        let value: String = rest
            .chars()
            .take_while(|c| *c != '\n' && *c != '│' && *c != '\u{1}')
            .collect();
        let value = value.trim().to_string();
        if !value.is_empty() {
            last = Some(value);
        }
    }
    last
}

/// Strip escape sequences so the transcript can be read (and matched) as text.
///
/// Cursor moves are replaced by a space rather than removed: without that,
/// `ratatui` writing "ROM" and "POKEMON" into two separate cells would read as
/// one word and every assertion here would be matching on an accident.
fn visible(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: parameters, then one final byte in @..~
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
                out.push(' ');
            }
            // OSC / APC / DCS: run to the string terminator.
            Some(']') | Some('_') | Some('P') => {
                chars.next();
                let mut prev = '\0';
                for c in chars.by_ref() {
                    if c == '\u{7}' || (prev == '\u{1b}' && c == '\\') {
                        break;
                    }
                    prev = c;
                }
                out.push(' ');
            }
            _ => {
                chars.next();
                out.push(' ');
            }
        }
    }
    out
}

/// A private copy of the vendored MIT ROM: the MBC writes a `.sav` beside the
/// file it is given, and the checkout must stay clean.
fn stage_rom() -> std::path::PathBuf {
    let src = std::path::Path::new("third_party/dmg-acid2/dmg-acid2.gb");
    let data = std::fs::read(src).expect("vendored dmg-acid2 ROM present");
    let dir = std::path::Path::new("target").join("basicterm");
    std::fs::create_dir_all(&dir).expect("create target/basicterm");
    let dst = dir.join("acid2.gb");
    std::fs::write(&dst, data).expect("stage ROM");
    dst
}
