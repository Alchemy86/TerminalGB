# Project agent memory

Durable, project-intrinsic knowledge for anyone — human or agent — working in this
repository: the architecture map, the standing conventions, and the non-obvious
constraints that bite.

It is deliberately **not** a changelog and not a results log. Two other places own
that material and both are indexed and gated:

- **[`docs/README.md`](docs/README.md)** is the complete documentation index — every
  page under `docs/`, what it is for, and the rule that keeps the index honest
  (`make docs-index`).
- **[`docs/conformance-notes.md`](docs/conformance-notes.md)** is the raw, chronological
  record of every emulation fix with its citations; **[`docs/measured/`](docs/measured/README.md)**
  is the same material reorganised by subject, and **[`docs/pitfalls/`](docs/pitfalls/README.md)**
  is the symptom-first index of traps in the tooling, the build and the harnesses.

If you are looking for *how to build, test, lint and contribute*, read
[`CONTRIBUTING.md`](CONTRIBUTING.md) first — this file assumes it.

## Layout

| Path | What it is |
|---|---|
| `src/` | The emulator core plus every host-side frontend. `cpu/ gpu/ mmu/ mbc/ link/` are the machine; `screen/` is the terminal, desktop and web frontends; `mcp/ portal/ scenario/ longplay/ music/` are optional host features behind cargo features. |
| `crates/terminalgb-plugin-api/` | The plugin contract. **Zero dependencies.** |
| `crates/` (others) | `terminalgb-chiptune` (drive the APU with no CPU), `terminalgb-gen1-battle` (a Gen 1 battle model), `loom-synth` (a general synthesis engine, unrelated to the Game Boy). |
| `plugins/` | In-process plugins, each its own crate. `example-tracer` is the one to copy. |
| `testharness/` | Integration harnesses and their checked-in baselines. Not `tests/` — that path is the test-ROM submodule. |
| `examples/` | Runnable evidence tools: scouts that score external suites, benches, probes. |
| `tools/` | Scripts that are not Rust: suite fetchers, cross-runners, doc generators, foreign-emulator oracles. |
| `platform/` | Console ports (`psp/`, `m5paper/`). **Not workspace members, and must never become them** — each needs a pinned nightly or a third-party toolchain that must not reach `make embed`, `cargo test` or `cargo install`. |
| `docs/` | Everything written down. `docs/tools/` holds the generators that keep it honest. |
| `third_party/` | Vendored artefacts, each with a lock and a README saying where it came from. |

`src/gameboy.rs` holds `Gameboy`, the public API; `src/lib.rs` holds the crate's
exports and the C ABI. **`Gameboy` and the `gameboy` module keep those names on
purpose**: the *product* is TerminalGB, the *machine it emulates* is a Game Boy.

## Feature axes

Read [`Cargo.toml`](Cargo.toml)'s `[features]` block — it is the authoritative,
commented list. The three facts worth knowing before you touch it:

- **`--no-default-features` is the embedding build**, and its dependency count is a
  contract: one third-party crate (`blip_buf`) plus first-party
  `terminalgb-plugin-api`. `make embed` asserts it, runs the core's tests, and runs
  `examples/headless` with `HOME`, `DISPLAY` and `WAYLAND_DISPLAY` unset. Anything a
  frontend needs goes behind a feature, never in the core.
- **`default` is the emulator and nothing cartridge-specific.** Everything Pokémon
  Generation 1 lives behind **`gen1`**. `make no-plugins` proves the emulator builds,
  lints and passes its library tests with every plugin off, and CI greps the
  dependency graph to prove no cartridge-specific crate reaches a plain build.
  Adding a plugin crate means adding it to that grep.
- **`audio` is off by default** so a fresh clone needs no ALSA. The APU is *always*
  emulated; only host playback is gated.

`make lint` runs clippy twice — `default` and `--features gen1` — because a bundle
leaving `default` would otherwise silently shrink what is linted. `make test` runs
`--features gen1`, so it covers the whole tree; `make no-plugins` is the other half,
proving the emulator still stands with every plugin off.

## Standing conventions

### Every emulation fix feeds the conformance tool

When you fix an emulation bug, capture it as a conformance test case **in the same
change**, because the reasoning is most available then and evaporates afterwards.
Record four things in the commit message and on the relevant
[`docs/components/`](docs/components/README.md) page:

1. **The observable symptom in user terms** — what a player would see or hear. That is
   what a foreign emulator with the same bug exhibits, so it is how a tool detects it.
2. **Which test ROM or check catches it.** *If nothing in the suite does, say so
   explicitly* — a coverage gap is itself a finding and a candidate check to add.
3. **The hardware behaviour being violated, with a citation** (Pan Docs, gbdev, a
   test-ROM readme). This is the explanation a tool hands its users.
4. **Any new test the fix produced**, flagged as a candidate to carry into the
   standalone [conformance tool](docs/conformance.md).

A few honest lines per fix, not a process document.

### A baseline is somebody else's judgement, never our own output

Every checked-in baseline records the **ROM's own verdict** — a serial string, the text
it writes to cartridge RAM, its author's reference image, its Fibonacci register dump —
and never a hash of a framebuffer we produced. That was the harness's original central
defect: four ROMs were pinned as hashes of our own output, so CI would have stayed green
straight through a regression in them.

- **Any change is red in either direction.** A row that starts *passing* is a baseline
  change too, and must be re-blessed deliberately with its reason in the commit.
- **Never re-bless a baseline to make a number look better.** If a change moves a
  result, say why. If an audio hash moves, listen to the WAVs before re-blessing and
  report the *properties* (RMS, peak, pitch, the ROM's own verdict) that did or did not
  move — a hash alone is not evidence.
- **Some rows are pinned red on purpose** and their headers say why.
  `blargg/oam_bug@cgb` is the standing example: the OAM-corruption defect is DMG
  silicon, so a real Color console fails that ROM too. Do not "fix" it.
- **Every baseline file names the engine that produced it and the target that
  re-blesses it**, because the two picture engines score the same ROMs differently and
  the obvious command rewrites the file with the other machine's numbers.

### Measure, do not quote

A number in a document is a hypothesis. Suite scores move most days; performance figures
belong to the box, the build profile and the picture engine that produced them. Re-run
before quoting, state the picture engine and the load average, and prefer an interleaved
A/B over two absolute readings taken minutes apart.

This applies to prose in this repository too: a claim in a doc that contradicts the
source is worth correcting rather than building on. Check the baseline, not a commit
message, before starting work on a named gap.

### Licence boundary: facts and formats are ours, assets are not

[`CONTRIBUTING.md`](CONTRIBUTING.md) has the contributor-facing rule. The two extensions
that are specific to this project's subject matter:

- **Never commit game assets — including as a test fixture.** Sprite data, tile sheets,
  music, text corpora, and hashes of any of them. Extract them at run time from the
  user's own ROM, or from a path they supply, and skip the check when it is absent.
  Addresses, structure layouts, character maps and compression formats are **facts**:
  usable freely, and cited by symbol name so a reviewer can check them.
- **Screenshots of our own interface are documentation; pictures of game assets are
  not.** The test: our interface, doing something, with the game incidental. A picture
  whose *subject* is the asset fails it even with our chrome around it — a whole-number
  magnification of a sprite is losslessly reversible, so it is the asset. Ship the
  command that renders it instead.

Reference implementations of other emulators are **read for behaviour and cited, never
copied**, unless their licence permits reuse and you say so at the call site. Check the
`LICENSE` file yourself and record what you found; do not rely on a remembered answer.

### The documentation is generated and gated

- `make docs-links` rewrites every `path:line` reference in `docs/` into a GitHub link
  and restamps each page's "documented as of `<sha>`" marker. **Run it after editing
  source that shifts line numbers**, then `make docs-check`.
- `make docs-check` (in CI) verifies links, in-range line numbers, the generated score
  markers, the picture tables and the documentation index. **It does not verify in-page
  `#anchor` links** — check those by hand after moving a heading.
- `make status` regenerates every baseline-derived score on `docs/status.md`, its HTML
  twin and the inline markers on the pages that quote one. **Re-blessing any baseline
  requires `make status` in the same commit.**
- `docs-check` only proves a `path:line` reference is *in range*. When a file is
  rewritten, references can point at the wrong content while staying in range —
  re-anchor them by content.

## Architectural invariants

These are the constraints that are expensive to rediscover.

**`[profile.release] panic = 'abort'`, and `Drop` is not a shutdown hook.** A panic is
process death *and* the loss of any unwritten cartridge RAM — the worst outcome an
emulator has. Two consequences: cartridge-supplied input must never be able to panic
(a mapper divide-by-zero, an out-of-range bank, a short or foreign `.sav` — hardware has
no such trap, so behave as hardware does), and nothing that must survive a shutdown may
be written in a destructor. The desktop event loop calls `process::exit`, so the
`Gameboy` inside it is never dropped; battery saves are written by an autosave on the
frame path with an explicit flush on close. The same rule covers shared-memory objects
and any other resource a crash would leak.

**Nothing on the emulator's frame path may block.** `AudioPlayer::play_samples` runs
inside `Cpu::do_cycle`, so anything it waits on is charged to the emulated frame; a
blocking hand-off once cost 17.9 ms against a 16.74 ms budget, with 97.6 % of it blocked
rather than executing. The same applies to every media sink: a configured FIFO or camera
opens non-blocking and retries, writes are non-blocking, and a slow consumer **loses
frames, never frame time**. Never put a second clock inside the emulator's inner loop.

**A machine with no `AudioPlayer` has no sound chip, not a quiet one.** `Sound::do_cycle`
returns immediately when no player is installed, so every `$FF10-$FF3F` read answers
`$FF` and every write is dropped — and a cartridge can tell. Real games hang on it. Any
host that drives a real cartridge must install one (`sound::SilentPlayer` costs nothing);
`testharness/blackout.rs` gates a named list of such hosts, and adding a host means
adding it to that list.

**Frame pacing is a deadline accumulator, never a reset clock.** The DMG frame is
70224 cycles / 4.194304 MHz = **16.743 ms**, not 16. Advance the deadline by exact
periods with bounded catch-up; resetting a pacing timer to `now()` silently charges draw
and poll overhead to the period and runs the machine slow.

**`savestate::FORMAT_VERSION` must be bumped on any layout change**, and a version
outside `MIN_SUPPORTED_VERSION..=FORMAT_VERSION` is refused rather than corrupting. The
writer always writes one current layout; only the reader is version-aware, upgrading
older states field by field. A *non-additive* change — a field removed, reordered, or
reinterpreted under the same layout — must raise the floor in the same commit. Machine
*configuration* (console revision, picture engine, speed flags) is deliberately **not**
serialised: a state loads into whatever machine is running.

**The plugin layering is the only acyclic arrangement, not tidiness.** The frontend
draws panels for the plugins it ships with, so `terminalgb` depends on the plugin
crates; if the `Plugin` trait lived in `terminalgb` the plugins would depend back and
Cargo would refuse the cycle. `terminalgb-plugin-api` declares what a machine must
provide (`MachineAccess`), `src/plugin.rs` implements it, and `Machine` is defined over
the trait. **Never move the trait back, and never let a plugin crate depend on
`terminalgb` outside `[dev-dependencies]`** (that back-edge is allowed and used — see
`terminalgb::testing::TestMachine`). The same argument produced `terminalgb-chiptune`
and `terminalgb-gen1-battle` as dependency-free crates.

**Capabilities are enforced per call, not trusted.** A plugin declares
`OBSERVE / READ_MEMORY / WRITE_MEMORY / CONTROL_INPUT / SERIAL_LINK`; an undeclared
`peek` returns `None` and an undeclared `poke` returns `false`. That makes "which plugin
wrote to my save?" answerable, and it is an interlock on the *emulated machine* — **not
a sandbox**. Plugins are trusted code the user compiled in. At most one may hold
`SERIAL_LINK`: a cable has two ends. Plugin state is not machine state and is not
serialised.

**A frontend is not a plugin.** A `Plugin` is handed a machine during a frame the
emulator has already decided to run. Anything that must *drive* the clock, replace the
cartridge, or own host resources is a frontend over `Gameboy::with_machine` — the MCP
server, the scenario runner and the parallel-instance drivers all reached that
conclusion independently.

**The core's host surface is enumerable, and that is the embeddability claim.** One
environment read (`$GB_MODEL`, and only to resolve `Model::Auto`), one wall-clock read
(only for an MBC3 clock cartridge's battery file), and `<rom>.sav` only if the caller
passed a path. Everything else — config files, `$XDG_*`, terminals, displays, video
devices — lives in `screen/`, `video.rs` and the audio backend. Adding a host dependency
to `cpu`/`mmu`/`gpu`/`mbc`/`sound`/`savestate`/`mode`/`upscale` breaks the claim.
`--no-default-features --features embedded` goes further and builds `no_std`.

**Two picture engines, two names.** The user-facing modes are `standard` (whole-scanline)
and `identical` (per-dot pixel FIFO, the default); the Rust variants are still
`RenderMode::{Fast, Accurate}`, and both spellings parse everywhere. They score the same
ROMs differently and each has its own baselines. `identical` refuses every speed flag.
Any figure quoted without naming the engine is ambiguous.

**A speed flag is a named shortcut, never a second code path.** `SpeedFlags` is a struct
of named bools and "speed mode" is a preset over it, so there is no parallel
implementation to fall out of step. A flag earns the preset by *measurement* against real
cartridge runs, not by argument. The suite runners refuse to score anything with
`$GB_SPEED` set: a conformance row measured on a shortcut machine is a claim about a
machine nobody will read it as describing.

**Never size a transmitted image by the window.** The terminal render path transmits at
the picture's own resolution and lets the terminal scale; sizing by the display rectangle
re-encodes megabytes per frame. Related, and all three are in
[`docs/rendering.md`](docs/rendering.md): never put a large escape sequence in a
`ratatui` cell symbol (the diff treats its byte length as a display width and re-emits
the screen), and reuse one image id rather than uploading a new image per frame.

**Detect, adapt, inform — never refuse over a missing capability.** A terminal without
the kitty keyboard protocol, without a graphics protocol, or without a cell-size report
must still run, degraded, and say so. The only legitimate refusal is neither a screen nor
a keyboard. A missing optional report must cost only the thing it reports.

**A panel is sized from its own contents.** Hard-coding a row count beside a `push` has
clipped a live plugin's status row more than once. The same rule for captions: shorten
through complete shorter forms, never clip — a truncated measurement cannot be told from
a complete one.

## Build and tooling traps

- **`cargo test` intermittently fails with `requires panic strategy 'abort' ... strategy
  of 'unwind'`, or with trait/duplicate-crate errors that make no sense.** This is a
  stale-artefact problem, not your source: `[lib] crate-type = ["cdylib", "rlib",
  "staticlib"]` makes three targets share one output path and `panic = 'abort'` gives two
  incompatible rlibs. `touch src/lib.rs` and rebuild, or build every test target in one
  invocation. Full symptom catalogue in [`docs/pitfalls/builds.md`](docs/pitfalls/builds.md).
  **Do not "fix" it by removing `panic = 'abort'`** — see the invariant above.
- **A `harness = false` test must not link a plugin crate the library also links**, for
  the same reason. Talk to `terminalgb`'s re-exports instead.
- **`pkill -f` and `pgrep -f` match your own shell's command line.** Both directions
  have destroyed work here — a `pkill` that killed the command invoking it, and a
  `pgrep` false negative that made a healthy multi-hour run look dead. Kill by a PID you
  captured at launch, and check liveness with `pgrep -a`. This matters because the long
  runs in `examples/` are measured in hours.
- **The `tests/` submodule is not initialised in a fresh clone or worktree**, and the
  test-ROM harness panics without it. `git submodule update --init --depth 1 tests`
  before trusting any before/after comparison.
- **The scouts and harnesses are `cargo build` targets.** After deliberately breaking
  source for a teeth test, rebuild before trusting any later measurement — a stale binary
  looks exactly like a regression.
- **Never point a test at the user's ROM directory.** The MBC writes a `.sav` next to the
  ROM. Stage the cartridge into `target/` first.
- **Inserting an item in a large file can silently move the attribute above it.** A
  function dropped between an orphaned doc comment and its `#[cfg]` takes the gate.
  `cargo clippy --all-targets` on a minimal feature set is what catches it.

## Verifying documentation renders

Render it through GitHub's own renderer and look at it:

```bash
python3 -c "import json;print(json.dumps({'mode':'gfm','context':'<owner>/<repo>','text':open('README.md').read()}))" > /tmp/md.json
gh api -X POST /markdown --input /tmp/md.json > /tmp/body.html
```

Serve the result from the repository root so relative image paths resolve as they do on
GitHub, wrap it in `<article class="markdown-body">` with an explicit
`<meta charset="utf-8">` (without it every em dash renders as mojibake and the source
looks corrupt), and screenshot it. Caveats: `mode=gfm` renders the input as a *comment*,
so newlines become `<br>` and paragraphs look hard-broken — judge images and layout from
the preview, not line wrapping — and it emits no heading `id` attributes, so in-page
anchors always look broken there. Delete the preview artefacts before committing.

Two rules learned from generated score markers, which are HTML comments inside prose:
**never put one inside a heading** (it buries an unguessable anchor) and **never let one
start a line** — an HTML comment at the start of a line opens a CommonMark HTML block
that runs to the next blank line, so the rest of the paragraph renders literally. A
wrapped list item is the easy way to walk into it.

## Maintaining this file

Keep this file for knowledge useful to almost every future session in this project.

- **Do not repeat what the codebase already shows.** Point at the authoritative file,
  command, or doc instead. `Cargo.toml`'s feature comments, `CONTRIBUTING.md`,
  `docs/README.md` and the component pages are all better homes for their own subjects.
- **Results belong in `docs/`, not here.** A fix's record goes in
  `docs/conformance-notes.md` at the moment of the fix and is distilled into
  `docs/measured/` when a subject accumulates enough of them. A tooling trap goes in
  `docs/pitfalls/`. This file holds the *invariants*, not the diary.
- Prefer rewriting or pruning an existing entry over appending a new one.
- Keep it proportionate: skip edits for changes that produced no durable knowledge.
