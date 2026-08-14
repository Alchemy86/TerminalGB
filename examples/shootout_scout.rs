//! Score this emulator the way the [GB Emulator Shootout] scores one, against
//! its own ROMs and its own reference screenshots.
//!
//! ```text
//! git clone https://github.com/gbdev/GBEmulatorShootout
//! python3 tools/shootout/manifest.py GBEmulatorShootout > manifest.tsv
//! cargo run --release --features image --example shootout_scout -- manifest.tsv
//! ```
//!
//! **Why a scout and not the real thing.** The shootout runs an emulator as a
//! Windows GUI process and screenshots its client area with `pyautogui`; that
//! is the only measurement that counts, and `docs/shootout.md` records the
//! end-to-end check of it. What this gives you is the same *verdict* for all
//! 264 ROMs in under a minute on any machine, which is what makes the number
//! something anyone can re-derive before quoting it. The two agree because the
//! comparison is the same one:
//!
//! * PIL's `convert("L")` luma, `L = 0.299R + 0.587G + 0.114B`;
//! * `util.py:compareImage` — equal iff **no pixel differs by more than 50**,
//!   which is what lets our `000000/606060/c0c0c0/ffffff` DMG palette match
//!   references drawn in `000000/555555/aaaaaa/ffffff`;
//! * `Emulator.run`'s ~10 Hz polling with early exit, and its frame budget;
//! * `Test.getDefaultResult` — a ROM with no reference is **INFO**, not a
//!   failure, and INFO counts toward the headline score.
//!
//! The console comes from the manifest, i.e. from the shootout's own test
//! definitions — not from the cartridge header. That matters: nine Mooneye
//! tests print their boot registers, so running a DMG row on a Color console
//! fails it however correct the CPU is.
//!
//! Sibling of `examples/mooneye_scout.rs`, `examples/samesuite.rs`,
//! `examples/mealybug.rs` and `examples/cpp_suite.rs`. Nothing is vendored:
//! the ROMs and the references are other people's, fetched at run time.
//!
//! [GB Emulator Shootout]: https://gbdev.io/GBEmulatorShootout/

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use terminalgb::gameboy::Gameboy;
use terminalgb::mode::Model;
use terminalgb::sound::SilentPlayer;

const W: usize = 160;
const H: usize = 144;

/// `util.py:compareImage` — a per-pixel greyscale tolerance, not equality.
const TOLERANCE: u8 = 50;

/// The shootout polls the screen every 100 ms, i.e. roughly every sixth frame,
/// and exits as soon as a reference matches.
const POLL_EVERY: u32 = 6;

/// Nothing is compared before the emulator has had a moment to draw anything —
/// the runner's own `startup_time`, in frames.
const SETTLE_FRAMES: u32 = 30;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    Pass,
    Fail,
    Info,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Info => "INFO",
        }
    }
    /// The shootout's headline score counts everything that is not `FAIL`
    /// (`main.py`'s sort key, `build.py`'s totals). `INFO` is a test with no
    /// correct answer — `acid/which.gb` asks the machine which console it is —
    /// so it is not a failure for anybody.
    fn counts(self) -> bool {
        self != Verdict::Fail
    }
}

struct Row {
    name: String,
    model: Model,
    frames: u32,
    rom: PathBuf,
    pass_refs: Vec<PathBuf>,
    fail_refs: Vec<PathBuf>,
}

/// PIL `convert(mode="L")`.
fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000).min(255) as u8
}

fn load_ref(path: &Path) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.to_rgb8();
    if img.width() as usize != W || img.height() as usize != H {
        eprintln!(
            "warning: reference {} is {}x{}, not {W}x{H} — ignored",
            path.display(),
            img.width(),
            img.height()
        );
        return None;
    }
    Some(img.pixels().map(|p| luma(p[0], p[1], p[2])).collect())
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= TOLERANCE)
}

fn frame_luma(gb: &Gameboy) -> Vec<u8> {
    let d = gb.image();
    (0..W * H)
        .map(|i| luma(d[i * 4], d[i * 4 + 1], d[i * 4 + 2]))
        .collect()
}

fn parse_model(s: &str) -> Model {
    match s {
        "DMG" => Model::Dmg,
        "CGB" => Model::Cgb,
        // The shootout's three SGB rows are the ICD2 command channel, which is
        // all a cartridge can read back — see `docs/components/sgb.md`.
        "SGB" => Model::Sgb,
        other => panic!("unknown model {other:?} in the manifest"),
    }
}

fn read_manifest(path: &Path) -> Vec<Row> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read manifest {}: {e}", path.display()));
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        assert!(
            f.len() >= 6,
            "manifest line has {} fields, expected 7: {line}",
            f.len()
        );
        let refs = |s: &str| -> Vec<PathBuf> {
            s.split(',')
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .collect()
        };
        rows.push(Row {
            name: f[0].to_string(),
            model: parse_model(f[1]),
            frames: f[2].parse().expect("frame budget"),
            rom: PathBuf::from(f[3]),
            pass_refs: refs(f[4]),
            fail_refs: refs(f[5]),
        });
    }
    rows
}

/// Copy the ROM somewhere private first. The MBC writes `<rom>.sav` beside the
/// image it was loaded from, and the ROMs here live in somebody else's git
/// checkout — the same rule `testharness/testroms.rs` follows.
fn stage(work: &Path, rom: &Path, index: usize) -> PathBuf {
    let name = rom.file_name().unwrap_or_default();
    let dst = work.join(format!("{index:03}")).join(name);
    std::fs::create_dir_all(dst.parent().unwrap()).expect("work dir");
    std::fs::copy(rom, &dst)
        .unwrap_or_else(|e| panic!("cannot stage {}: {e}", rom.display()));
    dst
}

fn run_one(row: &Row, staged: &Path) -> Verdict {
    let pass_refs: Vec<Vec<u8>> =
        row.pass_refs.iter().filter_map(|p| load_ref(p)).collect();
    let fail_refs: Vec<Vec<u8>> =
        row.fail_refs.iter().filter_map(|p| load_ref(p)).collect();

    let data = std::fs::read(staged).expect("staged rom");
    let mut gb =
        match Gameboy::try_new_with_model(data, Some(staged.to_path_buf()), row.model) {
            Ok(gb) => gb,
            Err(e) => {
                eprintln!("  {}: cannot load: {e}", row.name);
                return Verdict::Fail;
            }
        };
    // A real machine always has an APU, and several of these ROMs poll `NR52`
    // or read PCM12/PCM34. Without one they are not measuring a quiet emulator,
    // they are measuring one with no sound chip — see `sound::SilentPlayer`.
    gb.enable_audio(Box::new(SilentPlayer));

    for f in 0..row.frames {
        gb.frame();
        if f >= SETTLE_FRAMES && f % POLL_EVERY == 0 {
            let cur = frame_luma(&gb);
            if pass_refs.iter().any(|r| same(&cur, r)) {
                return Verdict::Pass;
            }
            if fail_refs.iter().any(|r| same(&cur, r)) {
                return Verdict::Fail;
            }
        }
    }
    if pass_refs.is_empty() {
        Verdict::Info
    } else {
        Verdict::Fail
    }
}

/// `blargg/cpu_instrs/01-special.gb` → `blargg`, for the per-suite table.
fn suite_of(name: &str) -> &str {
    name.split('/').next().unwrap_or(name)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut manifest = None;
    let mut out = None;
    let mut only: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next(),
            "--only" => only = it.next(),
            "-h" | "--help" => {
                eprintln!(
                    "usage: shootout_scout <manifest.tsv> [--only <substring>] \
                     [--out <results.tsv>]\n\n\
                     Build the manifest with:\n  \
                     python3 tools/shootout/manifest.py <GBEmulatorShootout> \
                     > manifest.tsv"
                );
                std::process::exit(2);
            }
            other => manifest = Some(other.to_string()),
        }
    }
    let manifest = manifest.unwrap_or_else(|| {
        eprintln!("usage: shootout_scout <manifest.tsv> [--only <s>] [--out <f>]");
        std::process::exit(2);
    });

    let rows = read_manifest(Path::new(&manifest));
    let rows: Vec<Row> = match &only {
        Some(s) => rows.into_iter().filter(|r| r.name.contains(s)).collect(),
        None => rows,
    };

    let work =
        std::env::temp_dir().join(format!("shootout-scout-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("work dir");

    let mut results = Vec::new();
    let mut per_suite: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let started = std::time::Instant::now();
    for (i, row) in rows.iter().enumerate() {
        let staged = stage(&work, &row.rom, i);
        let v = run_one(row, &staged);
        let e = per_suite.entry(suite_of(&row.name)).or_insert((0, 0));
        e.1 += 1;
        if v.counts() {
            e.0 += 1;
        }
        println!("{:3}/{}  {}  {}", i + 1, rows.len(), v.label(), row.name);
        results.push((row.name.clone(), v));
    }
    let _ = std::fs::remove_dir_all(&work);

    let total = results.len();
    let scored = results.iter().filter(|(_, v)| v.counts()).count();
    println!("\n--- by suite ---");
    for (suite, (ok, n)) in &per_suite {
        println!("{suite:<28} {ok:>3} / {n}");
    }
    println!(
        "\nTOTAL {scored} / {total} not-FAIL   ({:.1} %)   in {:.1} s",
        100.0 * scored as f64 / total.max(1) as f64,
        started.elapsed().as_secs_f64()
    );

    if let Some(path) = out {
        use std::fmt::Write as _;
        let mut body = String::new();
        for (n, v) in &results {
            let _ = writeln!(body, "{n}\t{}", v.label());
        }
        std::fs::write(&path, body).expect("write results");
        println!("wrote {path}");
    }
}
