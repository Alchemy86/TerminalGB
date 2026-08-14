# `gen1-link` — a virtual link-cable partner for Generation 1

**The emulator becomes the Game Boy on the other end of the cable.** Walk into any Cable Club
in Pokémon Red or Blue, use the console, and trade with a partner that does not exist — or
fight it in the Colosseum.

```bash
cargo build --release --features plugin-gen1-link
GB_PLUGINS=gen1-link GB_GEN1_TRADE=~/mewtwo.trade \
  cargo run --release --example terminal -- ~/roms/blue.gb
```

With no trade file it offers the level 5 event Mew.

## It is not a memory poke

It declares `SERIAL_LINK | OBSERVE | READ_MEMORY` and **not** `WRITE_MEMORY`, so the host would
refuse a write even if the code attempted one. The Pokémon arrives the way any traded Pokémon
arrives: the game asked for it on the wire, ran its real link protocol, and inserted it itself.

**The proof is a number we never sent.** The partner puts a level 5 Mew on the wire with
**125** experience — the exact total for level 5 on Mew's growth curve. Read it out of the
party afterwards and it has **135**. Nothing in the plugin knows that number: Generation 1
gives a *traded* Pokémon boosted experience, and the game applied it on the receiving side, of
its own accord, because it believed it had traded with a person. A memory poke would have left
125 sitting there. That single number is checked on every run of
[`examples/gen1_trade.rs`](../../examples/gen1_trade.rs).

## Things to preserve

Written down because each cost real time to find:

- **Answer `$01` with `$02` and the game takes the clock.** `ESTABLISH_CONNECTION_WITH_*` and
  `USING_*_CLOCK` share values, so the reply *is* the role assignment. Being the slave is why
  this needs **no timing tolerance at all** — the partner answers synchronously inside the
  `SC = $81` write. Every hardware or network bridge has to solve the clocked-timing problem;
  we sidestep it by choosing the other role.
- **Each block costs `N + 1` transfers.** `Serial_ExchangeBytes` re-sends its first byte until
  it receives `$FD`, so the stream leads every block with one extra preamble byte: **644 bytes**
  (18 + 425 + 201) for blocks of 17 / 424 / 200. Off by one and the party block shifts.
- **`AddEnemyMonToPlayerParty` copies the 44 bytes verbatim.** Nothing is recomputed after a
  trade, so stats, experience-for-level, types, catch rate and PP must all be right *on the
  wire*. Multi-byte fields are big-endian; the HP DV is assembled from the low bits of the
  other four.
- **The patch list is not optional.** `$FE` is the protocol's "no data" byte and the receiver
  *skips* it, so every `$FE` in the 264-byte mon region is swapped for `$FF` and its 1-based
  offset recorded in a two-part list split at 252. Silent corruption otherwise. A name cannot
  contain `$FE` — the digit `8` — and that is refused with a clear error.
- **The Cable Club needs the LEFT console tile, facing RIGHT** (`engine/link/cable_club.asm`).
  The right tile is the other Game Boy's seat, and the left player is the one the game makes
  clock master — the role this plugin wants it to have. A run that hangs at "Please wait…" is
  almost always the wrong seat.

## No game data is embedded

Base stats, the dex↔index mapping, species names and move PP are located in **the player's own
ROM** by byte signature and cross-checked (Pikachu, THUNDERBOLT) before use. Mew's base stats
live *outside* the main table in Red/Blue and are handled explicitly.

## The load-bearing abstraction

`PokemonSource` ([`spec.rs`](src/spec.rs)) keeps *where the Pokémon comes from* separate from
*how it is traded*. A file, a converter, a remote peer and a live partner all become
interchangeable — which is what makes cross-generation and remote trading possible later
without touching the protocol.

## Layout

| file | what it holds |
|---|---|
| [`protocol.rs`](src/protocol.rs) | the byte-level link state machine |
| [`party.rs`](src/party.rs) | the 44-byte Pokémon structure, the 418-byte party block, the patch list |
| [`romdata.rs`](src/romdata.rs) | base stats, species and move tables, read out of the player's cartridge |
| [`battle.rs`](src/battle.rs) | the Colosseum (link battle) path |
| [`spec.rs`](src/spec.rs) | `PokemonSource` and the trade-file format |
| [`text.rs`](src/text.rs) | the Gen 1 character encoding |

## Tests

```bash
cargo test -p terminalgb-plugin-gen1-link            # 59 unit tests, no cartridge needed
cargo test --release --test colosseum             # link battle against a real cartridge (skips without one)
cargo run  --release --example gen1_trade         # the whole trade, headless, end to end
```

`gen1_trade` also takes `--shots <dir>` (a PNG per stage transition) and `--state <path>` (a
save state at the console the instant before the trade), which is how the screenshots in
[`docs/link-trade/`](../../docs/link-trade/) are reproducible.

The `colosseum` harness is the regression for a crash that once scribbled 4,378 bytes over a
battery save: it asserts the game never leaves its own code **and the save file is never
touched**.

📖 **[The full story and what to do if it hangs → `docs/link-trade.md`](../../docs/link-trade.md)**
· [the Colosseum → `docs/link-battle.md`](../../docs/link-battle.md)
· [driving the cable from outside → `docs/gen1/link-from-outside.md`](../../docs/gen1/link-from-outside.md)
