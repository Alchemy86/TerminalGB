// The terminal frontend, kept as an example so existing workflows —
// `cargo build --release --example terminal`, and every launcher script built
// on that path — keep working. The real source is the installable binary at
// `src/bin/terminalgb.rs`; this file includes it so the two cannot drift.
include!("../../src/bin/terminalgb.rs");
