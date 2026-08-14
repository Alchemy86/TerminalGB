# Chiptune renderer — instructions in, real Game Boy audio out

**A composer can hear exactly what Game Boy hardware would produce, without owning
hardware.** Feed in notes, a `.vgm` log or raw register writes; get a `.wav` back.

The thing on the other end is not a synthesiser imitating a Game Boy. It is
[this repository's emulated APU](components/apu.md), unmodified — the same chip the
accuracy suites are scored against, driven the same way a cartridge drives it. That is
the whole value: the answer is authoritative rather than approximate.

```bash
cargo run --release --bin terminalgb-chiptune -- docs/chiptune-examples/demo.gbscore -o demo.wav
```

---

## 1. How good is the chip? — the number the claim rests on

| suite | what it measures | score |
|---|---|---|
| [SameSuite](https://github.com/LIJI32/SameSuite) `apu/` | APU corner cases — trigger delays, envelope, sweep, LFSR, DIV-APU phase | **64 / 69** |
| SameSuite, whole directory | above plus `dma/`, `ppu/`, `sgb/` | **71 / 76** |
| Blargg `dmg_sound` | the register-visible APU, twelve sub-tests | **12 / 12** |
| Blargg `cgb_sound` | the same on Color hardware | **12 / 12** |

Every one of those is the **test ROM's own verdict** — SameSuite's `RESULT_CODE` at
`$CFFE`, Blargg's printed status — never a hash of our own output. Check them yourself:

```bash
make test-external                      # scores SameSuite against a pinned copy
cargo test --release --test testroms    # Blargg dmg_sound / cgb_sound
python3 -c "rows=[l.split('=') for l in open('testharness/samesuite_baseline.txt') if l.strip() and not l.startswith('#')]; apu=[v for k,v in rows if k.strip().startswith('apu/')]; print(sum(v.strip().startswith('PASS') for v in apu), '/', len(apu))"
```

For context on how hard that suite is: its own README records that SameBoy — the
reference implementation — fails two of these tests. See
[`docs/components/apu.md` §8](components/apu.md) for the five we do not pass and why,
and [`docs/conformance-notes.md`](conformance-notes.md) §10-16 for the per-fix record.

**What this does *not* claim.** It is not a recording of a physical Game Boy. Two
things are deliberately not modelled: the console's analogue output stage (the DMG's
amplifier and speaker colour the sound, and the headphone output differs from the
speaker), and the five SameSuite rows above. Where the emulated chip and real silicon
are known to differ, that page says so.

---

## 2. What you feed in — three layers, one ground truth

The design decision, made deliberately: **support register writes, because that is the
layer that cannot be wrong, and put exactly one small friendly layer on top of it.**

```
   score  ──┐
   .vgm   ──┼──►  Vec<RegisterWrite>  ──►  emulated APU  ──►  samples  ──►  .wav
   .gbregs──┘      (the ground truth)
```

Every front end compiles to the same `Track` of `(cycle, register, value)`, and
`--dump-registers` prints it back out in a form the tool re-reads. So you can always
see what the chip is actually being told, and you can always drop to that level.

### 2a. `.gbregs` — register writes with timing

The honest hardware interface: `NR10`-`NR52` and wave RAM, each at an exact cycle.
This is what a cartridge puts on the bus.

```
model dmg
0t      NR52 $80      # power on — nothing else lands while the APU is off
+0t     NR50 $77      # master volume 7 both sides
+0t     NR51 $FF      # every channel to both sides
+0t     NR11 $80      # 50% duty
+0t     NR12 $F0      # volume 15, envelope off
+0t     NR13 $D6      # period low  — 2048 - 131072/440 = 1750 = $6D6
+0t     NR14 $86      # trigger + period high
+1s     NR12 $00      # DAC off: the note ends
end +1s
```

Times are absolute unless prefixed `+`, which makes them relative to the previous line.
`1234t` is T-cycles of the 4,194,304 Hz clock; `250ms`, `1.5s` and (after a `tempo`
line) note values like `1/8` all work. Values take `$hex`, `%binary` or decimal.

Full example: [`docs/chiptune-examples/tone.gbregs`](chiptune-examples/tone.gbregs).

### 2b. The score — notes, and one construct per hardware feature

Deliberately **not** a tracker language. A whole music language would be a second
project and would put a translator between the composer and the chip, which is the
thing this tool exists to remove. So: one construct per hardware feature, named after
the hardware feature, plus a `raw` escape hatch straight down to the registers.

```
tempo 132
model dmg

ch1 duty 2              # 0=12.5% 1=25% 2=50% 3=75%
ch1 env 15 - 3          # volume 15, counting down, period 3
ch1 note C4 1/8
ch1 note E4 1/8
ch1 note G4 1/4
ch1 sweep 3 - 2         # channel 1 only: period, direction, shift
ch1 note C5 1/2

ch2 vol 12              # constant volume = `env 12 0 0`
ch2 note C3 1/2

ch3 wave 0123456789ABCDEFFEDCBA9876543210   # 32 four-bit samples
ch3 level 2             # 0=mute 1=100% 2=50% 3=25%
ch3 note C3 1/1

ch4 noise 4 15 4        # shift, LFSR width (15 or 7), divisor
ch4 hit 1/16
ch4 rest 1/16
```

**Each channel keeps its own cursor**, starting at time zero and advancing by each note
or rest *it* plays. That is what makes four-part writing fall out with no bar or pattern
concept: write channel 1's melody, then channel 2's bass, and they line up because both
started at zero. `ch1 at 2s` repositions a cursor if you need it.

| line | what it does |
|---|---|
| `tempo <bpm>` · `model dmg\|cgb` · `title <text>` | piece-wide |
| `master <left 0-7> <right 0-7>` | NR50 |
| `power on\|off` | emit the power-on preamble, or don't (default on) |
| `tail <duration>` · `end <duration>` | trailing silence / explicit total length |
| `write <time> <reg> <value>` | one register write at an absolute time |
| `chN note <name> <duration>` (or just `chN C4 1/8`) | a note |
| `chN hit <duration>` | channel 4's note — noise has no pitch |
| `chN rest <duration>` · `chN at <time>` | move the cursor |
| `chN duty 0-3` | NRx1 pulse width (channels 1, 2) |
| `chN vol 0-15` · `chN env <vol> <+\|-\|0> <period 0-7>` | NRx2 |
| `ch1 sweep <period> <+\|-> <shift>` · `ch1 sweep off` | NR10 |
| `ch3 wave <32 hex digits>` · `ch3 level 0-3` | wave RAM, NR32 |
| `ch4 noise <shift> <15\|7> <divisor>` | NR43 |
| `chN pan left\|right\|both\|off` | that channel's NR51 bits |
| `chN raw <register> <value>` | anything not covered, at the cursor |

Durations: `1/4`, `3/8`, dotted `1/4.` or `1/8..`, `250ms`, `1.5s`, `1234t`. Notes:
`C4` is middle C, `A4` is 440 Hz; `C#4`, `Db4` and `A-4` all parse.

Full example: [`docs/chiptune-examples/demo.gbscore`](chiptune-examples/demo.gbscore).

### 2c. `.vgm` — the established chiptune log format

**Investigated, and recommended: yes, and it turned out to be the cheapest of the
three.** VGM is the format the chiptune world already logs to, it has had a Game Boy
variant since v1.61, and there are large public archives of Game Boy VGM files. More to
the point, **a VGM is already a timed register log**, so importing one involves no
interpretation at all: the spec says command `0xB3 aa dd` writes `dd` to Game Boy
register `aa`, and "register 00 equals GameBoy address FF10". That is a direct mapping
onto `Track`, about 150 lines, and it makes the tool immediately useful with material
people already have — which no bespoke syntax could.

We also **export** VGM (`--vgm out.vgm`), which matters for a different reason: it lets
anyone play a phrase composed here through a second, independent implementation and
compare. A renderer that can only be checked against itself is not checkable.

Scope, stated plainly:

* **`.vgz` (gzipped VGM) is not read.** It is detected by its magic and reported with
  the one-line fix (`gunzip`), rather than failing as a corrupt header. A DEFLATE
  decoder is not worth a dependency in a crate whose boast is that it has none.
* **Only the Game Boy chip is played.** A multi-chip VGM's other writes are skipped
  *correctly*, by their documented operand lengths, so the Game Boy part's timing stays
  exact — and they are counted, so the importer can say how many it ignored rather than
  silently rendering half a piece.
* A VGM's clock is 44,100 ticks per second by definition of the format, where the APU's
  is 4,194,304. Write times therefore quantise to ~95 T-cycles on the way in. That is
  the format's resolution, not ours.

---

## 3. Using it

### As a command

```bash
terminalgb-chiptune <input> [-o out.wav] [--rate 44100] [--model dmg|cgb]
                 [--seconds N] [--dump-registers] [--vgm out.vgm] [--info]
terminalgb-chiptune --example       > phrase.gbscore   # a score to start from
terminalgb-chiptune --example regs  > phrase.gbregs    # the register layer
```

The format is detected from the content, so the extension does not matter, and `-`
means standard input or output (`… - -o - | aplay` works).

`--info` says what was parsed. `--dump-registers` prints the compiled register writes —
the ground truth — and renders nothing.

### As a library

```rust
use terminalgb::chiptune::{render, RenderOptions};
use terminalgb_chiptune::{score, wav};

let track = score::compile("tempo 120\nch1 env 15 - 3\nch1 note C4 1/4\n")?;
let samples = render(&track, RenderOptions::default());   // Vec<(f32, f32)>
wav::write(std::path::Path::new("out.wav"), &samples, 44_100)?;
```

`terminalgb_chiptune::compile_auto(bytes, model)` sniffs any of the three formats.
`terminalgb::chiptune::EmulatedApu` is the chip if you want to drive it yourself, and
`EmulatedApu::read` reads a register back exactly as the CPU would — which is how you
show that a length counter really did switch a channel off.

### From an MCP client

The `render_audio` tool ([`docs/mcp.md`](mcp.md)) takes a `score` string or a `path`,
writes a WAV, and reports what it produced: length, peak level, which channels sounded,
and — with `"registers": true` — the compiled register writes. It needs no cartridge.

An agent cannot hear the file, and the tool says so: the reply is written to be the
feedback loop instead. `Peak 0.000` with a warning naming the three usual causes is far
more use to a model than a file it cannot open.

---

## 4. Determinism

**Same input, same bytes.** Integer time throughout — durations are computed as exact
rationals and rounded once, into T-cycles — no clock is read, and the one piece of
ambient state the APU would otherwise pick up from the environment
(`$GB_CGB_REVISION`) is pinned by the renderer. `testharness/chiptune.rs` asserts it
rather than assuming it, and asserts the converse too: a changed note must change the
samples, or the check has no teeth.

---

## 5. Where it lives, and why

**Not in the emulator core.** Someone embedding the core in hardware should not inherit
a music renderer, and `--no-default-features` is the build that guarantees it: the
chiptune crate is an optional dependency and is simply absent there. CI's "the core has
exactly one third-party dependency" gate is measured on that build and is unmoved.

Two crates, split the way the plugin system is split and for exactly the same reason:

* **[`crates/terminalgb-chiptune/`](../crates/terminalgb-chiptune)** — the music half:
  score, `.vgm` and register-script parsing, the `Track` IR, the WAV encoder, and an
  `Apu` **trait**. **Zero dependencies**, and it does *not* depend on `terminalgb`.
* **[`src/chiptune.rs`](../src/chiptune.rs)** — sixty lines implementing that trait over
  `terminalgb::sound::Sound`, behind the `chiptune` cargo feature.

That direction is not tidiness, it is the only acyclic arrangement: the MCP server and
the CLI live in `terminalgb`, so `terminalgb` must be able to depend on the chiptune crate,
so the chiptune crate must not depend back. `Apu` is the seam that makes it possible —
the same job `MachineAccess` does for plugins ([`docs/plugins.md`](plugins.md)).

It is *not* a plugin. A `Plugin` is handed a machine during a frame the emulator has
already decided to run; this drives a bare sound chip with no CPU, no PPU and no
cartridge in the picture at all. The plugin contract would have been the wrong shape,
and using it would have implied a running console that is not there.

### The WAV encoder is shared

`terminalgb_chiptune::wav` is where `testharness/audiocapture.rs`'s private RIFF writer
now lives — byte for byte, because `testharness/audio_baseline.txt` pins an FNV hash of
the interleaved PCM. There is a unit test asserting the 44-byte header field by field
so a later edit cannot quietly move that baseline, and the audio harness was re-run
before and after the move to confirm every hash was unchanged. The music-mode work
(`fm/gameboy-musicmode-m5`) consumes the same module; that was agreed rather than
discovered, so there is one encoder in the workspace and not three.

---

## 6. Proving it — what was measured

`cargo test --release --test chiptune` — 27 checks, no ROM, no audio device, ~2 s, in
CI. Each one measures a property of the **rendered audio** against a number from Pan
Docs, not against our own previous output. The interesting ones:

**Pitch is the chip's, not equal temperament.** A C major scale renders at
261.61, 293.87, 329.32, 349.51, 392.40, 439.83, 494.62, 524.30 Hz — worst error **0.14
cents** from what the 11-bit period register can express. Those are not the equal
tempered frequencies: C5 is 523.25 Hz in theory and **524.288 Hz** on a Game Boy,
because `2048 - 131072/523.25` rounds to 1798 and there is no finer tuning. Telling a
composer that is most of what they came for.

**The envelope is a 64 Hz counter.** Volume 15 counting down with period 3 steps every
**46.9 ms** measured, against 3/64 s = 46.875 ms from hardware, fifteen times, reaching
silence at 706 ms against 703 ms predicted.

**The sweep operates on the period register, and can kill the channel.** A downward
sweep from C5 falls 524 Hz → 88 Hz in 100 ms. An upward sweep with shift 3 climbs for
one sweep period and then the channel goes **completely silent** — because the period
register overflowed past 2047 and the hardware switched it off. Nothing about a pitch
envelope bolted onto a tone generator predicts that; it is a specific rule in the sweep
unit, and it is how the classic vanishing "power-up" is made.

**The noise channel is an LFSR, not a random-number generator.** With NR43 shift 4,
divisor code 0, the LFSR is clocked at 32,768 Hz. In 7-bit mode it repeats every 127
output bits — measured autocorrelation **0.995 at 3.878 ms**, against 127/32768 =
3.876 ms from hardware, and −0.013 at one and a half times that lag. In 15-bit mode,
same clock, the sequence is 32,767 bits (a full second) and the same measurement gives
**−0.009**: no periodicity at all. One bit of NR43 is the difference between a metallic
rattle and a hiss, and it is exactly right.

**Duty is the square channels' whole timbre.** 50% duty produces **odd harmonics only**
(2nd and 4th are 40 dB down, 3rd and 5th are −9.5 and −14 dB). 25% loses every fourth
harmonic; 12.5%'s first null is the eighth; and 25% and 75% have **identical magnitude
spectra**, as they must, being each other's inverse.

**DMG and CGB really differ.** Load a length counter while the APU is powered off: a
DMG keeps it (length counters live outside the powered domain) and the channel switches
itself off 3.9 ms after the trigger; a CGB discards the write and the same score plays
on for 250 ms. Same score, same tool, one flag — and the renderer reads `NR52` back to
show it.

### Why `model` is two machines when the emulator now models nine

The emulator distinguishes DMG, DMG0, MGB, SGB, SGB2, CGB, CGB0, CGB-E and AGB. Almost all
of that difference is **boot state** — which registers the boot ROM leaves behind at
`PC=$0100` — and a score is not a cartridge: it starts from a cold, powered-off chip and
brings the APU up itself, exactly as a music driver does. So the nine collapse to the two
things the *sound synthesis* can tell apart:

* **`model dmg` vs `model cgb`** — the length-counter and wave-RAM rules
  (`Sound::set_mode`), which is what the check above measures.
* **CGB CPU revision** — a genuinely separate axis: revisions 0 and B clock an extra length
  step where C and later do not (`CgbRevision`, `docs/components/apu.md` §8). The renderer
  pins **CGB-C** rather than reading `$GB_CGB_REVISION`, because a render has to be
  reproducible; `RenderOptions.cgb_revision` is the knob if you want an older machine.

If a ninth machine ever gains an APU difference a program can hear, this is where it goes —
not in a longer `model` list for its own sake.

### The renderer tracks the emulator's accuracy — demonstrated by accident

While this work was being rebased, an APU fix landed on `main`:
`7556dde fix(apu): a square's frequency reload is latched late (SameSuite 71->72)`. Rendering
the same demo score before and after is the cleanest possible demonstration of what this tool
is for:

| section of the demo | samples changed | largest change |
|---|---:|---:|
| the C major scale | **0.0 %** | 0 |
| the arpeggio | **0.0 %** | 0 |
| the falling sweep | 1.8 % | 0.52 |
| the rising sweep | **43.4 %** | 0.48 |

Plain notes are untouched; only the swept ones moved — which is exactly what "a square's
frequency reload is latched late" should affect, because the sweep unit is the only thing
rewriting `NR13`/`NR14` in the middle of a note. A composer who rendered that phrase
yesterday now gets a *more correct* answer, with the difference confined to the part of the
music the fix is about. That is the property a synthesiser cannot have.

It is also the reason this page quotes a suite score rather than a promise: the claim moves
with the emulator, so it has to be re-measurable.

### What it sounds like

The evidence above is measurement, and it should be said plainly that measurement is
what it is: the spectra, the LFSR period and the envelope staircase were computed from
the rendered samples, and a spectrogram of the demo was inspected — eight ascending
harmonic stacks for the scale, a smooth glissando fanning down for the sweep, a stepped
fade for the envelope, broadband speckle for the 15-bit noise and a comb of discrete
lines for the 7-bit noise, which is the picture of "hiss" versus "metallic rattle".
Everything is where a Game Boy puts it and nothing looks like a bug.

To listen for yourself:

```bash
cargo run --release --bin terminalgb-chiptune -- docs/chiptune-examples/demo.gbscore -o /tmp/demo.wav
paplay /tmp/demo.wav          # or aplay, ffplay, or open it in anything
```

No audio is committed to the repository: the scores are two kilobytes and regenerate
the WAVs exactly, and everything here was written for this tool — nothing sampled,
transcribed or borrowed.

---

## 7. Sharp edges

* **`Sound::do_cycle` returns immediately when no player is installed**, so a harness
  that forgets to attach one has *no APU at all*, not a quiet one. `EmulatedApu`
  attaches a capture player in its constructor; there is a check in the harness that a
  silent track really is silence at the full sample rate.
* **The APU may not be advanced by large steps.** It hands its resampler a block when
  its internal clock crosses the block period — once per call, because it is written to
  be driven by a CPU that never advances it by more than one instruction. 1024 T-cycles
  already overflows the resampler's buffer. `terminalgb_chiptune::STEP_CHUNK` is 64, three
  times the longest Game Boy instruction. Chunking is otherwise invisible: the chip
  evaluates its channels lazily over exact time ranges, so where the boundaries fall
  does not change a single sample.
* **Rendering runs `FLUSH_TAIL_CYCLES` (~62 ms) past the track's end** so the final
  partial sample block is handed over; the samples are then truncated to the exact
  length. Nothing a listener hears changes, but a caller reading chip *state* after a
  render is looking at the machine that much later than the track's end.
* **The renderer places writes on exact cycles, which is something a cartridge cannot
  do**: a real program's writes land on instruction boundaries. That makes the register
  layer strictly more expressive than a Game Boy program, not less accurate — but a
  score that depends on sub-instruction placement is not something you could ship in a
  ROM.
* **Nothing here writes `DIV`.** The frame sequencer is driven by the system counter,
  and a game that resets `DIV` shifts the 512 Hz phase (SameSuite's `div_*` group is
  about exactly that). A score has no way to ask for it; a `.vgm` cannot express it
  either. If it is ever wanted, `Sound::div_write` is the hook.
