# The trade portal client — asynchronous friend trades over HTTPS

The portal is a web app (its own repository, built in parallel) where two
friends each offer a Pokémon and accept each other's; the emulator then arms
its **local** [gen1-link partner](plugins.md) with the Pokémon the friend is
sending, and the trade happens at the Cable Club exactly like every other
virtual-partner trade. Nothing is synchronised: each player's cable is answered
locally, so both sides are instant and nobody waits. The design comes from the
portal scout report (two HTTP calls instead of a 1.5k–2.2k-transfer live relay,
whose wall-clock cost is `transfers × RTT`).

The emulator half is `src/portal/` behind the `portal` cargo feature (in
`default`; excluded from `--no-default-features` and wasm, so `make embed` is
untouched).

## Using it

```toml
# config.toml
portal  = https://portal.example      # $GB_PORTAL overrides
plugins = gen1-link
```

1. **Sign in once** — `@` inside the TUI (picker or game), or `terminal login`.
   The browser opens on the portal's approval page; the URL and a code like
   `WXYZ-1234` are printed too, so a headless or SSH session signs in the same
   way. `terminal logout` forgets the stored token.
2. **Offer / accept on the website.** The emulator only *consumes* matched
   trades in v1 (the offer/accept client calls exist — `Portal::offer`,
   `friend_offers`, `accept` — but no UI drives them yet).
3. **Load the game.** If a trade is pending, the attach note and the `Portal`
   status row say so, and the link partner is armed with the friend's Pokémon.
   Walk into the Cable Club → TRADE CENTER and trade.
4. **Completion is confirmed automatically** (`POST /api/trades/<id>/done`,
   observed on the partner's own `trades_completed()` counter), with bounded
   retries and a loud give-up message naming the website as the manual path.

Manual end-to-end walk (also the integration tool against the real portal):

```bash
GB_PORTAL=<url> cargo run --release --example portal_trade -- <rom.gb> [--done]
```

## What never happens

- **No save, no ROM, no game bytes are ever uploaded.** The only thing on the
  wire is a ~250-byte *specification* of one Pokémon (the JSON spelling of
  `MonSpec`, `plugins/gen1-link/src/spec.rs`), resolved against the receiving
  player's own cartridge tables like every other `PokemonSource`.
- **A failure never falls back silently.** If the portal is configured but
  signed out, unreachable, or serving something malformed, the gen1-link
  partner is **not armed at all** — not even from a local trade file — because
  a matched trade could exist that this process cannot see, and trading a
  Pokémon away to the built-in partner while a friend waits is exactly the
  silent-loss failure a trade feature must not have. The only route to the
  trade-file fallback is the portal answering, signed in, "nothing pending".
- **The token never rides argv** (visible in `ps`): every request is described
  in curl config syntax on **stdin** (`curl --config -`). The stored token is
  `${XDG_STATE_HOME:-~/.local/state}/terminalgb/portal-token`, mode `0600`,
  written atomically (tmp → rename), and **fingerprinted to the portal URL it
  was issued by** — change `portal =` and the token is refused, not sent to
  the new server (`auth::parse_store`; the same rule as the remembered
  volume's `from_config`).

## The HTTP contract (the emulator's side of it)

The route list is the cross-repo agreement; the portal repo documents its side
as `docs/api-contract.md`. **The JSON bodies below are what this client sends
and accepts** — if the portal crew's shapes differ, one side must move, and
that is a coordination item, not something either side changes silently.

A **mon** object is `MonSpec` in JSON. Required: `species` (name, `#151`, or
index — resolved by `RomTables::resolve_species`), `level` (1–100), `ot_id`
(u16), `dvs` (`[atk, def, spd, spc]`, each 0–15; the HP DV is derived).
Optional: `nickname`, `ot_name` (default `PORTAL`), `moves` (≤ 4 names).
Out-of-range values are refused **by field name** at fetch time
(`client::mon_from_json`), before anything is armed.

```text
POST /api/device/code    {"client_id":"terminalgb"}
  → 200 {"device_code","user_code","verification_uri",
         "verification_uri_complete"?, "interval"?:5, "expires_in"?:900}

POST /api/device/token   {"client_id","device_code","grant_type":
                          "urn:ietf:params:oauth:grant-type:device_code"}
  → 4xx {"error":"authorization_pending" | "slow_down" | "access_denied"
         | "expired_token"}            (RFC 8628 names; any 4xx accepted)
  → 200 {"access_token","refresh_token"?,"user"?}
         ("access"/"refresh" also accepted)

GET  /api/trades/mine                                (Bearer auth from here on)
  → 200 {"trades":[{"id" (string or number), "friend"?, "give"? (display
         text), "trainer"? (in-game partner name; falls back to friend,
         uppercased), "receive": <mon>}]}

POST /api/trades/<id>/done   {}          → 200 {}

POST /api/offers             {"mon": <mon>}            → 200 {"id"}
GET  /api/friends/<user>/offers
  → 200 {"offers":[{"id","summary"?}]}
POST /api/offers/<id>/accept {"give": <mon>}           → 200 {"trade_id"|"id"}

401/403 on any authed route → "run terminal login again", never a retry loop.
```

Notes for the portal crew, from the client's behaviour:

- `interval` is honoured (and `slow_down` adds 5 s, per RFC 8628); the poll
  loop gives up at `expires_in`. The report sketched `428` for pending — this
  client accepts **any** 4xx whose body carries the `error` field.
- `verification_uri_complete`, if served, is what the browser is pointed at
  (code pre-filled); the plain URI + code are always shown as the fallback.
- The `user` field on the token reply is what "signed in as …" shows; without
  it the client still works and just says "signed in".
- Trade `id`s may be strings or numbers; the client echoes them back into the
  `/done` path as text.

## Why the transport is the system `curl` binary

Dependencies are a contract in this repo (`make embed` asserts the embedding
build has exactly one third-party crate), and the project's standing answer is
to hand-roll (PNG, inflate, deflate, base64, JSON). TLS is the one thing that
must never be hand-rolled, so the transport is borrowed from the OS instead of
a crate: `curl` ships on effectively every Linux/macOS box and on Windows
10+. If it is missing, every entry point says so by name up front
(`http::curl_available`). Every request carries `connect-timeout = 5` /
`max-time = 15` — an external request with no timeout can park the caller
forever. The WebRTC precedent in the scout report (rejected for being "a heavy
dependency … against a crate that guards its dependency count as a contract")
is the same judgement.

Dead end recorded: passing the JSON body with `--data` on argv was never an
option (the device code is a credential and argv is world-readable), and
curl's config-file quoting only understands `\\` and `\"` — `http::config_quote`
escapes exactly those and refuses control characters, and
`credentials_travel_in_the_stdin_config_with_timeouts_set` pins it.

## What is measured, what is inferred, what is unverified

**Measured against a local stub** (`testharness/portal.rs`, 16 checks — a real
`TcpListener` speaking the bodies above through the real curl transport; plus
the `terminal login`/`logout` CLI and `examples/portal_trade.rs` run by hand
against a Python stub with a real Pokémon Blue cartridge, where trade #71
resolved to "BULBA the BULBASAUR (Lv14)" against the cartridge's own tables
and the `/done` POST landed with the bearer token):

- the whole device-code flow, including two `authorization_pending` rounds,
  the browser hand-off (`$BROWSER` pointed at `true`), token storage, `0600`
  permissions, and the portal-URL fingerprint refusing a mismatched token;
- trade fetch, spec validation refusing an out-of-range DV **by trade id and
  field**, empty-list ⇒ `Ok(None)`, `401` ⇒ an auth error naming
  `terminal login`;
- completion confirm reaching `/api/trades/71/done` authenticated;
- an unreachable portal failing in bounded time (the curl timeouts).

**Unverified until the real portal is live:** that the portal serves these
exact field names (the harness is this client talking to itself, honestly
labelled); HTTPS specifically (the stub is plain HTTP — the TLS half of curl
is curl's, but no request in this repo has crossed a TLS boundary);
`verification_uri_complete` and `slow_down` (accepted by code, never served
by the stub — unit-level only); token refresh (`POST /api/token/refresh` is
in the sketch but this client does not call it yet — an expired token surfaces
as the loud 401 message); and the in-TUI `@` flow end-to-end in a real
terminal (its state machine is the same `LoginFlow` the harness drives, but
nobody has pressed `@` in a live TUI against a portal).

## Follow-ups this leaves open

- **Offering from the TUI** ("offer this one" from the trainer panel) — the
  client calls exist, no UI drives them.
- **Token refresh** — silent renewal instead of the loud re-login message.
- Multiple pending trades arm only the **oldest**; the description says how
  many are waiting, and the rest arm on later loads after the first confirms.
