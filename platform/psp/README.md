# TerminalGB on a PlayStation Portable

```sh
./tools/psp-build.sh          # → target/psp/EBOOT.PBP
```

Copy it to `ms0:/PSP/GAME/terminalgb/EBOOT.PBP` and put cartridges in
`ms0:/PSP/GAME/terminalgb/roms/`. It appears in the XMB as **TerminalGB**.

Measurements, the honest limits, and what it would take to reach 59.7275 Hz:
**[`docs/portability.md` §3](../../docs/portability.md)**. Short version: it plays Pokémon
Blue at ~42 fps under PPSSPP, exactly (every frame hashes the same as the desktop build),
and nobody here has tried it on real hardware.

## Controls

| | |
|---|---|
| D-pad, Cross, Circle | the Game Boy's d-pad, A and B |
| Start, Select | Start and Select |
| Triangle | toggle nearest / linear filtering |
| L, R | save / load `<rom>.gbss` |
| Home | quit |

In the ROM picker: Up/Down, then **Cross, Circle or Start**. Under PPSSPP the keyboard `X`
key is *Circle* and Cross is on `Z` — see `docs/portability.md` §3.6, it reads as a broken
build if you do not know.

## Why this is not a workspace member

It needs Rust nightly, `-Z build-std` and a third-party `std` overlay for a target upstream
Rust supports only as `no_std`. None of that may reach `make embed`, `cargo test` or
`cargo install terminalgb`, so this directory is outside the workspace and is built only by
`tools/psp-build.sh`, which pins the nightly and the overlay commit.

`src/main.rs` uses nothing but the public embedding API — `Gameboy::try_new_with_model`,
`frame`, `image`, `keydown`/`keyup`, `save_state`/`load_state`, `flush_battery` — the same
surface `examples/headless.rs` demonstrates. Nothing in `src/` knows a PSP exists.
