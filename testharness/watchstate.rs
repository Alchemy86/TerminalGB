//! Does watching a save state being built change the save state?
//!
//! `shiny_hunt --make-state` walks a brand-new Pokémon Blue to the grass on
//! Route 1 and saves the machine there; `--show` runs **the same walk** with
//! the picture drawn and the frames paced so a person can follow it. The
//! promise attached to that flag is that the two produce the **same bytes** —
//! and a promise about determinism is exactly the kind that rots quietly, since
//! a state that is subtly different still loads, still hunts, and simply is not
//! the state anyone measured.
//!
//! So this harness runs the real example binary twice, once each way, and
//! compares the files byte for byte. The watched run gets a **pty** with this
//! harness on the far end of it, answering the capability questions a terminal
//! answers — because `--show` refuses to start without a terminal, and a test
//! that ran it headlessly would be testing the flag it is meant to be watching.
//!
//! Two things are checked besides the bytes:
//!
//! * the watched run must actually have **drawn** something (an image escape or
//!   a `▀`-sampled picture reaching the pty) — otherwise "identical" would be
//!   satisfied by a `--show` that silently did nothing;
//! * neither run may leave a `.sav` beside the cartridge. `make_state` builds
//!   its machine with no file path precisely so it cannot, and that is a
//!   property worth a check rather than a comment.
//!
//! **It needs your own cartridge and skips with exit 0 without one**, like
//! `testharness/colosseum.rs`: no ROM is distributed with this repository.
//!
//! ```text
//! cargo build --release --example shiny_hunt
//! POKE_ROM=<blue.gb> cargo test --release --test watchstate
//! ```

use std::io::Read;
use std::os::unix::io::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The pty this harness presents. Wide enough that the caption box and the
/// picture both have room; the assertions do not read either.
const COLS: u16 = 120;
const ROWS: u16 = 40;

/// A watched run at `--show-speed max` is only as slow as the emulator, but the
/// walk holds on a few captions deliberately, so allow generously.
const RUN_TIMEOUT: Duration = Duration::from_secs(180);

fn main() {
    let Some(rom) = find_rom() else {
        println!("watchstate: no cartridge — set POKE_ROM=<Pokémon Blue or Red>.");
        println!("watchstate: SKIP (exit 0; a skip is not a pass)");
        return;
    };
    let Some(exe) = find_example() else {
        println!(
            "watchstate: examples/shiny_hunt is not built — run\n  \
             cargo build --release --example shiny_hunt"
        );
        println!("watchstate: SKIP (exit 0; a skip is not a pass)");
        return;
    };

    // Stage the cartridge into `target/`. The MBC writes a `.sav` next to the
    // ROM it is given a path for, and a harness must never put one next to
    // somebody's collection — see AGENTS.md.
    let dir = PathBuf::from("target/watchstate");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create target/watchstate");
    let staged = dir.join("cartridge.gb");
    std::fs::copy(&rom, &staged).expect("stage the cartridge");

    println!("watchstate: {}", staged.display());
    let mut failures = Vec::new();

    let headless = dir.join("headless.gbss");
    match run_headless(&exe, &staged, &headless) {
        Ok(()) => println!("  [1] headless --make-state ....... ok"),
        Err(e) => failures.push(format!("[1] headless --make-state: {e}")),
    }

    let watched = dir.join("watched.gbss");
    let mut drew = false;
    match run_watched(&exe, &staged, &watched) {
        Ok(bytes_drawn) => {
            drew = bytes_drawn > 0;
            println!(
                "  [2] --show on a pty ............. ok ({bytes_drawn} picture bytes)"
            );
        }
        Err(e) => failures.push(format!("[2] --show on a pty: {e}")),
    }

    // [3] The claim.
    match (std::fs::read(&headless), std::fs::read(&watched)) {
        (Ok(a), Ok(b)) if a == b && !a.is_empty() => {
            println!("  [3] the two states are identical  ok ({} bytes)", a.len())
        }
        (Ok(a), Ok(b)) => failures.push(format!(
            "[3] watching changed the state: {} bytes headless vs {} watched, \
             first difference at {:?}",
            a.len(),
            b.len(),
            a.iter().zip(&b).position(|(x, y)| x != y)
        )),
        (a, b) => failures.push(format!(
            "[3] a state is missing: headless {:?}, watched {:?}",
            a.map(|v| v.len()),
            b.map(|v| v.len())
        )),
    }

    // [4] "Identical" must not be satisfied by a `--show` that drew nothing.
    if drew {
        println!("  [4] the watched run drew a picture  ok");
    } else {
        failures.push("[4] --show produced no picture at all".into());
    }

    // [5] Neither run may write beside the cartridge.
    let strays: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sav") || n.ends_with(".gbsave") || n.ends_with(".srm"))
        .collect();
    if strays.is_empty() {
        println!("  [5] no save file beside the ROM ... ok");
    } else {
        failures.push(format!("[5] a run wrote {strays:?} beside the cartridge"));
    }

    if failures.is_empty() {
        println!("\nwatchstate: all checks passed");
        return;
    }
    println!();
    for f in &failures {
        println!("watchstate: FAIL {f}");
    }
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// The two runs
// ---------------------------------------------------------------------------

fn run_headless(exe: &Path, rom: &Path, dest: &Path) -> Result<(), String> {
    let out = base_command(exe, rom, dest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// The same run with `--show`, on a pty this harness owns.
///
/// Returns the number of bytes of *picture* the frontend put on the wire — an
/// image protocol's escape sequence, or the `▀` half-block glyphs. Zero means
/// the flag drew nothing and the identity check below is vacuous.
fn run_watched(exe: &Path, rom: &Path, dest: &Path) -> Result<usize, String> {
    let (master, slave) = open_pty(COLS, ROWS)?;

    let mut child = unsafe {
        base_command(exe, rom, dest)
            .arg("--show")
            // As fast as the machine will go: the pacing is not what is under
            // test, and a real-time run would take three minutes.
            .arg("--show-speed")
            .arg("max")
            .stdin(Stdio::from_raw_fd_dup(slave)?)
            .stdout(Stdio::from_raw_fd_dup(slave)?)
            .stderr(Stdio::from_raw_fd_dup(slave)?)
            // Its own session with the pty as controlling terminal, so even a
            // `/dev/tty` probe lands here and nothing can reach the terminal
            // this test was launched from.
            .pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            })
            .spawn()
            .map_err(|e| format!("spawn: {e}"))?
    };
    unsafe { libc::close(slave) };

    let picture = Arc::new(Mutex::new(0usize));
    let done = Arc::new(AtomicBool::new(false));
    let reader = spawn_fake_terminal(master, picture.clone(), done.clone());

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(e) => return Err(format!("wait: {e}")),
        }
        if started.elapsed() > RUN_TIMEOUT {
            let _ = child.kill();
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    done.store(true, Ordering::Relaxed);
    unsafe { libc::close(master) };
    let _ = reader.join();
    let drawn = *picture.lock().unwrap();

    match status {
        Some(s) if s.success() => Ok(drawn),
        Some(s) => Err(format!("exit {:?}", s.code())),
        None => Err(format!("did not finish within {RUN_TIMEOUT:?}")),
    }
}

fn base_command(exe: &Path, rom: &Path, dest: &Path) -> Command {
    let mut c = Command::new(exe);
    // Isolate the run from the developer's own config, recents and saved
    // volume: a test must neither read nor write those.
    let home = PathBuf::from("target/watchstate/home");
    let _ = std::fs::create_dir_all(&home);
    c.arg("--rom")
        .arg(rom)
        .arg("--make-state")
        .arg(dest)
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_STATE_HOME", &home)
        // The shm handshake is not what is under test and costs a probe
        // timeout against a terminal that will never answer it.
        .env("GB_TERM_KITTY", "noshm")
        .env_remove("GB_TELEMETRY")
        .env_remove("GB_HUNT_VERBOSE");
    c
}

// ---------------------------------------------------------------------------
// Being the terminal
// ---------------------------------------------------------------------------

/// Read the child's output, answer the questions a terminal answers, and count
/// the bytes that are picture.
///
/// **Answer everything, including the queries we do not care about.** Silence
/// is not neutral: every unanswered query costs the frontend a multi-second
/// timeout in a blocking path, which reads exactly like a hang. (The lesson is
/// `testharness/basicterm.rs`'s, learned the hard way.)
fn spawn_fake_terminal(
    master: RawFd,
    picture: Arc<Mutex<usize>>,
    done: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut file = unsafe { std::fs::File::from_raw_fd(dup(master)) };
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 65536];
        while !done.load(Ordering::Relaxed) {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = &buf[..n];
                    *picture.lock().unwrap() += picture_bytes(chunk);
                    pending.extend_from_slice(chunk);
                    let replies = answers(&mut pending);
                    for r in replies {
                        use std::io::Write;
                        let _ = file.write_all(r.as_bytes());
                        let _ = file.flush();
                    }
                    // Never let the scan buffer grow without bound over a run
                    // that transmits megabytes of picture.
                    if pending.len() > 8192 {
                        let keep = pending.len() - 4096;
                        pending.drain(..keep);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    })
}

/// How much of `chunk` is a picture rather than chrome.
///
/// Sixel arrives as a DCS (`ESC P … ESC \`); a kitty or iTerm2 placement as an
/// APC or OSC 1337; half-blocks as `▀` glyphs. Any of the four is a picture.
fn picture_bytes(chunk: &[u8]) -> usize {
    const IMAGE_INTRODUCERS: &[&[u8]] = &[
        b"\x1bP",  // DCS — sixel
        b"\x1b_",  // APC — kitty graphics
        b"]1337;", // OSC 1337 — iTerm2 inline image
    ];
    if IMAGE_INTRODUCERS
        .iter()
        .any(|intro| find(chunk, intro).is_some())
    {
        return chunk.len();
    }
    // No image protocol on this connection: count the `▀` glyphs a half-block
    // picture is made of. U+2580 is `e2 96 80` in UTF-8.
    chunk.windows(3).filter(|w| *w == b"\xe2\x96\x80").count()
}

/// The replies a terminal gives, drained out of `pending` as they are matched.
///
/// Deliberately advertises sixel in the primary device attributes (`4`): it is
/// a protocol the frontend really can drive, so the watched run exercises a
/// real encoder rather than the do-nothing case.
fn answers(pending: &mut Vec<u8>) -> Vec<String> {
    const QUERIES: &[(&[u8], &str)] = &[
        // Primary device attributes: VT220 with sixel.
        (b"\x1b[c", "\x1b[?62;1;4c"),
        (b"\x1b[0c", "\x1b[?62;1;4c"),
        // Device status, cursor position.
        (b"\x1b[5n", "\x1b[0n"),
        (b"\x1b[6n", "\x1b[1;1R"),
        // XTWINOPS: text-area size in pixels, cell size, size in cells.
        (b"\x1b[14t", "\x1b[4;640;1200t"),
        (b"\x1b[16t", "\x1b[6;16;10t"),
        (b"\x1b[18t", "\x1b[8;40;120t"),
        // Kitty keyboard: not answered on purpose — this is not a kitty
        // terminal, and `--show` reads no keys it needs the protocol for.
    ];
    let mut out = Vec::new();
    loop {
        let mut hit = None;
        for (q, a) in QUERIES {
            if let Some(at) = find(pending, q) {
                hit = Some((at, q.len(), *a));
                break;
            }
        }
        // A kitty graphics query (`APC G … ST`) is swallowed unanswered — the
        // frontend appends a cursor-position fence to it precisely so silence
        // is a conclusive "no".
        let Some((at, len, reply)) = hit else { break };
        pending.drain(at..at + len);
        out.push(reply.to_string());
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

fn find_rom() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("POKE_ROM")?);
    p.is_file().then_some(p)
}

/// `target/<profile>/examples/shiny_hunt`, derived from this test binary's own
/// path — a `harness = false` test runs from `target/<profile>/deps/`.
fn find_example() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    for dir in me.ancestors() {
        let candidate = dir.join("examples").join("shiny_hunt");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn open_pty(cols: u16, rows: u16) -> Result<(RawFd, RawFd), String> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: cols * 10,
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
        return Err(format!("openpty: {}", std::io::Error::last_os_error()));
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
