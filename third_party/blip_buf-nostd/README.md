# `blip_buf` 0.1.5, made `no_std`

Upstream: <https://github.com/mvdnes/blip_buf-rs> — Mathijs van de Nes, **MIT**,
licence carried verbatim in [`LICENSE`](LICENSE). The crates.io release this was
taken from is the exact version [`Cargo.toml`](../../Cargo.toml) pins.

## Why it is here

`blip_buf` is the emulator's only third-party dependency and the APU cannot be
built without it — a machine with no sound chip is a *different machine*, not a
quiet one, and cartridges can tell (see `docs/measured/host.md`). The crate uses
`std` for exactly one thing, `Vec`, and never declares `#![no_std]`, so it
cannot be built for a bare-metal target as published.

## The diff, in full — two hunks

**One:** the `no_std` preamble.

```diff
+#![no_std]
+extern crate alloc;
+use alloc::vec;
+use alloc::vec::Vec;
```

**Two:** the crate's one call into the float library.

```diff
-        self.factor = factor.ceil() as fixed_t;
+        self.factor = ceil_u64(factor) as fixed_t;
```

`f64::ceil` is a `std` method — in `no_std` it comes from `libm`, and one call
is not worth a dependency. `ceil_u64` is the same value by integer arithmetic:
for `x >= 0`, `x as u64` truncates towards zero, so the ceiling is that plus one
whenever a fraction was dropped.

**That second hunk is checked, not asserted.** `ceil_u64_matches_std` sweeps
every clock/sample-rate pair the emulator can hand `set_rates`, plus 2,048
exact integers and the values a hair either side of each, and requires
agreement with the real `f64::ceil` on all 8,222 of them:

```sh
cargo test --manifest-path third_party/blip_buf-nostd/Cargo.toml
```

It has teeth. The first version of `ceil_u64` used a plain `+ 1` and the sweep
failed immediately with an overflow: an `as` cast from a float **saturates** at
`u64::MAX`, so upstream's `x.ceil() as u64` quietly returns the maximum where a
plain add panics. It is `saturating_add` for that reason.

Upstream's own two tests are in the file and still pass, so that command runs
three tests, not one. Reproduce the whole diff:

```sh
diff ~/.cargo/registry/src/*/blip_buf-0.1.5/src/lib.rs \
     third_party/blip_buf-nostd/src/lib.rs
```

Nothing else is changed — no constant, no signature, no resampling maths.

## Who uses it

**Only [`platform/m5paper`](../../platform/m5paper)**, through a
`[patch.crates-io]` in its own `Cargo.toml`. Every other build — `cargo build`,
`cargo test`, `make embed`, wasm — resolves `blip_buf` from crates.io exactly as
before and never sees this directory. It is not a workspace member.

## If upstream takes the change

Delete this directory and drop the `[patch.crates-io]` stanza. Nothing else
refers to it.
