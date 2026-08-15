# Build, target and toolchain pitfalls

← [Pitfalls index](README.md)

The compiler is usually right. These are the cases where it is right about something other
than what it says.

---

## Two builds of one crate, and rustc blames your code

**Symptom.** Any of these four, from a build that compiled yesterday:

```
error[E0277]: the trait bound `BattlePlugin: Plugin` is not satisfied
    = help: the following other types implement trait `Plugin`:
              terminalgb_plugin_battle_control::BattlePlugin
```

```
error[E0308]: mismatched types
    = note: `Track` and `terminalgb_chiptune::track::Track` have similar names,
            but are actually distinct types
    = note: perhaps two different versions of crate `terminalgb_chiptune` are being used?
```

```
/usr/bin/ld.bfd: undefined reference to `terminalgb::cpu::core::Cpu::do_cycle'
collect2: error: ld returned 1 exit status
```

```
the crate `X` requires panic strategy `abort` which is incompatible with
this crate's strategy of `unwind`
```

The first is the one that wastes the most time: **rustc's own `help` lists the exact type you
passed as implementing the exact trait it says is unimplemented.**

**The early warning, which everybody scrolls past:**

```
warning: output filename collision.
The lib target `terminalgb` in package `terminalgb v0.2.0 (…)` has the same output
filename as the lib target `terminalgb` in package `terminalgb v0.2.0 (…)`.
Colliding filename is: …/target/release/deps/libterminalgb.rlib
The targets should have unique names.
This may become a hard error in the future; see https://github.com/rust-lang/cargo/issues/6313
```

**What it looks like.** A broken refactor. You will re-read the `impl`, check the feature
flags, check the trait's generic bounds, and find nothing — because there is nothing wrong
with any of it. The note *"perhaps two different versions of crate X are being used?"* is
rustc telling you the truth and being ignored, because there is obviously only one version:
it is a path dependency in the same workspace.

**Root cause.** Two *builds* of one crate, not two versions.

Two things combine:

1. `[profile.release]` sets `panic = 'abort'` — but cargo **ignores** that setting for test and
   bench targets, which must be able to unwind. So a `--release` build that includes both a
   library consumer and a test target compiles the library **twice**, under two different
   panic strategies.
2. The library declares more than one crate type —
   `crate-type = ["cdylib", "rlib", "staticlib"]` in `Cargo.toml`. That makes cargo emit
   un-hashed artefact names into `target/<profile>/deps/`, so those two builds land on **one
   filename**: `libterminalgb.rlib`, `libterminalgb.so`, `libterminalgb.a`. Cargo says so
   itself, once per artefact, in the warning above — and links its own issue
   [rust-lang/cargo#6313](https://github.com/rust-lang/cargo/issues/6313).

Two builds of one crate produce genuinely distinct types, and whichever artefact a downstream
target happens to pick up decides whether its types match. Hence "similar names, actually
distinct types"; hence a trait `impl` that exists in one copy and not in the one you linked
against; hence undefined references at link time.

**Reproduced here, deterministically**, on a clean tree at `2e0da0e`:

| command | result |
|---|---|
| `cargo build --release --lib` | clean |
| `cargo build --release --examples` | clean |
| `cargo build --release --bins` | clean |
| `cargo build --release --tests` | clean — but prints 4 collision warnings |
| **`cargo build --release --all-targets`** | **fails**, 13 errors of the shapes above |
| `cargo clippy --all-targets` | clean, **no collisions at all** |

`clippy` escapes it because it uses the *dev* profile, which sets no `panic` key — one
strategy, one rlib, no competition. It is `--release` that splits the build in two.

**Resolution.**

* **Build target classes separately.** `--lib`, `--examples`, `--bins`, `--tests` each work.
  `--all-targets` in `--release` is the trigger.
* If you are already in the bad state, **force the library to rebuild**: `cargo clean -p
  <crate>` for the crate rustc named, or touch the lib source. Simply re-running the same
  command sometimes clears it and sometimes does not — the artefact on disk is what decides,
  not the source.
* **Do not "fix" it by removing `panic = 'abort'`.** That setting is load-bearing here: an
  abort is process death *and* the loss of any cartridge RAM not yet written, which is why
  guest input must never be able to panic the emulator
  ([`docs/measured/cartridges.md`](../measured/cartridges.md)). Removing it would change the
  failure mode of the whole product to fix a build ergonomics problem.
* The real fix is giving the library targets distinct names, which is a repo-wide change
  nobody has scheduled. Until then this is a known, documented sharp edge.

**Side effects and blast radius.** The one thing you must *not* do is treat this as a signal
about your change. It is an artefact of the artefact cache; the source is fine. Two specific
consequences:

* **A `harness = false` integration test must not link a plugin crate the library also
  links.** That is exactly this trap with a different face: the test resolves the plugin crate
  to a second (unwind-built) rlib, so a trait and its `impl` arrive from different crate
  instances. It compiled on the first run and failed on the next two. The fix was to have the
  harness talk only to re-exports from the main crate.
* **Anything that only appears in `--release` is a candidate.** If a failure reproduces in
  `--release` and not in dev, check the collision warning before you check your code.

**How we found it.** Recorded first as an intermittent `cargo test` failure, and reproduced
deterministically while writing this page — `cargo build --release --all-targets` on
`2e0da0e`, twice, with the collision warnings and all three error shapes in one run.

> **The general rule.** When rustc says two types with the same path are distinct, believe it,
> and go and look at how many copies of that crate your build graph contains. Look for a
> multi-`crate-type` library, a profile-level `panic` setting, or two feature resolutions of
> one dependency.

---

## A type check cannot see a missing 64-bit atomic

**Symptom.**

```
error[E0432]: unresolved import `std::sync::atomic::AtomicU64`
```

The whole emulator failed to compile for `mipsel-sony-psp`, on one line.

**What it looks like.** An exotic-target problem you can defer. It is not: the module in
question (`sound::host_status`, an audio-device error counter) is compiled on **every** build
including the minimal embedding one, so one line made the emulator unbuildable for an entire
class of target while every existing gate stayed green.

**Root cause.** A 32-bit target need not have 64-bit atomics, and MIPS32 does not —
`std::sync::atomic::AtomicU64` does not exist there at all.

**And note what did not catch it.** armv7 and i686 are both 32-bit and both build it fine,
because both *have* 64-bit atomics (`ldrexd`, `cmpxchg8b`). Pointer width is not the axis.
Neither `cargo check --target` on those, nor running on those, would have found it.

**Resolution.** `AtomicU32`, widened to `u64` on read so the published `Snapshot` type is
unchanged. A counter of audio-device errors cannot reach 2³² in a session.

**Side effects and blast radius.** The public type did not change, so nothing downstream
moved. The standing rule that came out of it: **do not put a 64-bit atomic in the core.** One
survives in `sound::cpal_audio`, and that is fine — that module is
`#[cfg(all(not(target_arch = "wasm32"), feature = "audio"))]` and never reaches a target
without the atomic. The rule is about *unconditionally compiled* code.

**How we found it.** By building for the real target. See
[`docs/portability.md`](../portability.md) §3.3.

> ### Cross-*checking* and cross-*running* are different claims
>
> `cargo check --target` proves the code type-checks against that target's `std` *as your
> toolchain models it*. It does not prove the target has the APIs you used, does not prove the
> program links, and does not prove it produces the right answer.
>
> The stronger claim is cheap here: `make test-cross` builds the embedding core for aarch64,
> armv7 and i686 (all **musl**, so rustup's own `libc.a` and `rust-lld` do the work and no C
> cross-toolchain is installed) and **runs a cartridge on each**, requiring a byte-identical
> frame hash and an identical save-state length against the host. A Game Boy emulator has a
> hundred ways to be nearly right and every one of them moves the frame — so frame identity
> across architectures is what makes a port *exact* rather than *approximately working*.
> `state bytes` matching is the separate claim that no `usize` leaked into the file format.
>
> And read [tests.md](tests.md#a-skip-is-not-a-pass) before quoting that gate: a target whose
> emulator is missing is skipped, and a run in which nothing ran still exits 0.

---

## Cargo cannot target-gate a `[[bin]]`

**Symptom.** A `wasm32` build fails inside a binary you never intended to build for wasm,
because its feature is in `default`.

**What it looks like.** A missing `cfg` on the module — but the module *is* gated; the
binary that imports it is what is not.

**Root cause.** There is no way to say "this `[[bin]]` exists only on these targets" in
`Cargo.toml`. If its feature is on by default, it is compiled.

**Resolution.** Give the binary a target-gated `main`:

```rust
#[cfg(target_arch = "wasm32")]
fn main() {}
```

**Side effects and blast radius.** None; the stub is unreachable on every other target.

**The sibling rule.** A `pub` accessor over a target-gated field needs a wasm arm that returns
the honest answer, not a `cfg` that deletes the accessor. `Gameboy::audio_enabled` returning
`false` on wasm keeps the API stable; removing it breaks every caller.

**How we found it.** Two breaks in the same hour, neither visible to GitHub CI — because this
project merges into a *local* `main` first, so CI runs on push and a wasm break on unpushed
commits is invisible for days. `make embed` now runs `make check-wasm` for exactly that
reason. The general form is in [`docs/measured/host.md`](../measured/host.md): **which build
you are running is part of the machine**, and no ROM-driven test can see it.
