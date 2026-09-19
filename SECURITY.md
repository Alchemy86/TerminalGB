# Security policy

TerminalGB is a Game Boy emulator, but it is not only a sandbox for old cartridges: it
opens a TCP link-cable port, it signs in to a web portal and stores a token, it exposes an
MCP server and a C ABI, and it parses files — ROMs, battery saves, save states — that its
user did not write. Those are real attack surfaces and this document says how to report a
flaw in one of them.

## Reporting a vulnerability

**Report privately. Do not open a public issue for anything exploitable.**

- Preferred: GitHub's [private vulnerability reporting](https://github.com/Alchemy86/TerminalGB/security/advisories/new)
  — the repository's **Security** tab → *Report a vulnerability*. It is a private thread
  between you and the maintainer, and it is where an advisory and a CVE are issued from if
  one is warranted.
- Alternatively, reach the maintainer through the contact on the
  [GitHub profile](https://github.com/Alchemy86).

A useful report names the build (`cargo` features matter a great deal here — see below),
the platform, the steps to reproduce, and what an attacker gets. A proof of concept is
welcome; a crashing input file attached to the report is ideal.

**What to expect.** An acknowledgement within 5 working days, and an assessment — in scope,
out of scope, or already known — within 10. This is a single-maintainer project, not a
product with an on-call rota; if a fix is going to take a while you will be told that
rather than left in silence. Fixes land on `main` and in the next release; you will be
credited in the advisory and the changelog unless you ask not to be.

Please give a reasonable window before public disclosure, and expect the same courtesy in
return: the advisory names the reporter, the defect, and the version that fixes it.

## Supported versions

The latest release and the current `main`. There are no maintained back-branches; a fix is
published forward, not backported.

## Scope

The features are individually gated, and **what you compile is what you expose** — a
`--no-default-features` embedding build has none of the network surface below. See the
`[features]` block in [`Cargo.toml`](Cargo.toml) for the authoritative list.

In scope:

- **The BGB link-cable server (`bgblink`, in the default build).** A `TcpListener` speaking
  a protocol defined by another emulator, driven by whatever connects to it. Anything a
  peer on that socket can do beyond exchanging link bytes — memory disclosure, a crash, a
  hang that survives disconnect, a bind that reaches further than the address the user
  configured — is in scope.
- **The trade portal client (`portal`, in `gen1`).** The device-code sign-in, the stored
  token (written `0600`, atomically, and refused when the portal URL it was issued for
  changes), and the handling of any response the portal returns. A malicious or
  impersonated portal that can reach outside the emulated machine, or leak the token, is
  in scope.
- **The MCP server (`mcp`).** It speaks the Model Context Protocol over stdin/stdout and
  exposes a *bounded* action surface; the memory-writing tool is behind an explicit
  `--allow-write-memory`. A client reaching host resources the server does not intend to
  offer, or escaping those bounds without the flag, is in scope.
- **The C ABI and the WebAssembly build** (`src/lib.rs`, `cdylib`/`staticlib`). These are
  `unsafe extern "C"` entry points taking caller-supplied pointers and lengths. Memory
  unsafety reachable from a *correct* caller — one honouring the documented contract — is
  in scope.
- **Parsing untrusted cartridge data.** A ROM, a `.sav` battery file, or a save state is
  data from somewhere else, and the release profile is `panic = 'abort'`: a panic is
  process death *and* the loss of any unwritten cartridge RAM. A file that panics the
  emulator, reads or writes out of bounds, or drives an unbounded allocation is a
  vulnerability here even though hardware would merely misbehave. Save states carry a
  version floor and are refused rather than reinterpreted; a state that bypasses that is in
  scope.

Out of scope:

- **Emulation inaccuracy.** A game that renders or sounds wrong is a bug — please
  [open an issue](https://github.com/Alchemy86/TerminalGB/issues) — but it is not a
  security report.
- **Plugin capability bypass by a plugin.** `OBSERVE / READ_MEMORY / WRITE_MEMORY /
  CONTROL_INPUT / SERIAL_LINK` are enforced per call, and that makes "which plugin wrote to
  my save?" answerable — but it is an interlock on the *emulated machine*, **not a
  sandbox**. Plugins are in-process Rust the user chose to compile in. A plugin doing
  anything a linked crate can do is the documented design.
- **Resource exhaustion the user asked for.** Fast-forward, headless speed and the
  long-horizon runner will use every cycle you give them.
- **Anything requiring an attacker who already has the user's local account**, including
  reading the token file or a save as that user.
- **Third-party services and vendored artefacts**, which have their own reporting paths.
  Report those upstream; tell us too if TerminalGB's use of them makes it worse.

## Deployment notes

The link server and the portal are conveniences for a person at a machine, not hardened
services. Bind the link port to an address you control — the documented examples use
`127.0.0.1` — and do not expose it to a hostile network. It authenticates nothing: anything
that connects is a Game Boy on the other end of the cable.
