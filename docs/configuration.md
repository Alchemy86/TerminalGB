# Configuration

The terminal frontend reads a small `key = value` file at
**`${XDG_CONFIG_HOME:-~/.config}/terminalgb/config.toml`**. A missing file means defaults; an
unknown key or an unusable value is **reported at start-up** and shown in the picker footer and
the `?` overlay rather than silently ignored. Values may be bare, `"double-quoted"` or
`'single-quoted'`.

Keys that are keystrokes rather than settings are on [the controls page](controls.md); which
cargo features a build has are on [build modes](build-modes.md).

---

## Every key

| Key | Values | Meaning |
|---|---|---|
| `rom_dir` | a path (`~` expands) | Directory the ROM picker browses. Default `~/roms`. |
| `protocol` | `auto` / `kitty` / `sixel` / `iterm2` / `halfblocks` | Force a graphics protocol. `auto` (default) asks the terminal. Pinning a value *disables* detection. |
| `palette` | `grayscale` / `green` / `amber` | Recolour DMG games. Default `grayscale`. |
| `upscale` · `upscale_factor` | `nearest` / `scale2x` / `mmpx` / `lcd` · an integer | [Pixel-art upscaler](upscaling.md) run each frame. |
| `cell_scale` | `auto` / a number ≥ 1.0 | HiDPI device-pixel-per-cell correction. Default `1.0`. |
| `ppu` | `standard` / `identical` | Which [picture mode](picture-modes.md). Default `identical`. `$GB_PPU` overrides for one run. |
| `volume` | `0`–`100`, or `off` | Playback volume. Default `50`. Applied to the samples handed to the sound card, never to the Game Boy's own `NR50`. |
| `shiny_alert` | `on` / `quiet` / `off` | How loudly a wild Pokémon's shiny-if-traded verdict is announced. |
| `key_a` / `key_b` / `key_start` / `key_select` | a single character | Rebind the buttons (movement is always the arrows). |
| `screenshot_dir` · `video_dir` · `music_dir` | paths (`~` expands) | Where `c`, `V` and music mode's `w` write. |
| `video_out` | `-` / a file / a FIFO / `/dev/videoN` / a directory / `off` | Where `v` sends the stream. Default `off`. Nothing is opened until you press `v`. |
| `video_autostart` | `true` / `false` | Go live at launch. Default `false` — naming a destination is not the same decision as sending frames to it. |
| `video_format` · `video_size` · `video_source` · `video_pace` | see [video-out.md](video-out.md) | Wire format, letterbox size, native-or-displayed picture, and pacing. |
| `plugins` | comma-separated names | Attach [plugins](plugins.md). **Empty by default — nothing loads implicitly.** |
| `gen1_trade` · `battles` | paths | Trade file for `gen1-link`; battle presets for `gen1-battle`. |
| `link` | `auto` (default) / `off` / a second ROM path | The [link cable](link-cable.md). `auto` is live by default. |
| `link_net` | `[listen:\|connect:]host:port` | Put the link port on a TCP socket, speaking [BGB's protocol](bgb-link.md). |
| `portal` | a URL, or `off` | The [trade portal](portal.md): asynchronous friend trades over HTTPS. |
| `agent_command` · `agent_objective` · `agent_max_calls` · `agent_max_usd` | see [claude-plays.md](claude-plays.md) | The agent driver, its objective, and its hard caps. |
| `telemetry` | a path | Opt-in local [NDJSON run data](telemetry.md). Off unless set. |

---

## Upgrading from before the rename

The project used to be called `gameboy`. **Nothing was moved and nothing needs moving:** the old
`~/.config/gameboy/` and `~/.local/state/gameboy/` are still read and written where they are,
because the emulator prefers the new name and falls back to the old one whenever the old one is
what exists on disk. That covers the config file, the recent-ROM list, the remembered volume, save
states, screenshots and recordings.

A migration that half-succeeds — or that runs while a second copy of the emulator is writing — is
worse than a fallback that always works, which is why there is no migration step.

---

## See also

* **[every control](controls.md)** — the full written key list, generated from the same registry
  the in-app panel renders from
* **[build modes and cargo features](build-modes.md)** — what a build contains, and the two
  run-time switches (`GB_PPU`, `GB_MODEL`) that are read once at start-up
* **[the quick start](quickstart.md)** — from a clone to a running emulator
