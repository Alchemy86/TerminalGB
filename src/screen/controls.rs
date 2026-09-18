//! **The one place every key the terminal frontend binds is written down.**
//!
//! Before this there were three lists and no relationship between them: the
//! in-app **Controls** panel, the **`?` overlay**, and whatever a feature page
//! happened to mention. A key could be added to the handler and forgotten by
//! all three, or added to one and not the others — and it was: `F2` and the
//! `Esc` that leaves fullscreen were bound and named nowhere at all.
//!
//! So this module is the registry, and three things are derived from it:
//!
//! 1. the **Controls panel** ([`panel_lines`]) — it renders from here, so the
//!    panel cannot list a key that does not exist or miss one that does;
//! 2. **[`docs/controls.md`](../../../docs/controls.md)** ([`markdown`]) — the
//!    written page, every row carrying a link to the document that explains the
//!    key properly, regenerated and gated by a test (see
//!    `the_controls_doc_matches_the_registry` in `screen::tui`);
//! 3. a test that the **`?` overlay** names every key in here
//!    (`the_help_overlay_names_every_key_in_the_registry`), which is how the
//!    overlay is kept honest without flattening its prose into a table.
//!
//! It is deliberately *data*, not behaviour: nothing here dispatches a key.
//! The handler in [`super::tui`] is still the only thing that decides what a
//! key does, and the registry is checked against it by the tests above rather
//! than trusted.
//!
//! **Terminal frontend only.** The desktop and web frontends bind the game pad
//! and nothing else; `docs/controls.md` says so and says where they live.

/// Where a control appears in the in-app **Controls** panel.
///
/// The panel is clipped from the *bottom* on a short window, so order is a
/// product decision, not a formatting one: an everyday control must not be the
/// first casualty. Volume therefore sits in the third row and the capture keys
/// — which a player can afford to lose — sit last.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    /// Not in the panel. Still in the `?` overlay and in the written page —
    /// the panel is a reminder, not the reference.
    Hidden,
    /// The game pad block at the top. The character is the player's, from
    /// `config.toml`, so the label cannot be a constant.
    Pad(Pad),
    /// Half a row; paired with the next `Half` in the same group.
    Half(&'static str),
    /// A row of its own.
    Wide(&'static str),
    /// A row of its own, and **only when there is a cable** — a key that does
    /// nothing is worse than no key at all. The label says which of the two
    /// things it will do, so it comes from the session, not from here.
    Cable,
}

/// A game-pad button, whose key is configurable and therefore not a constant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pad {
    Arrows,
    A,
    B,
    Start,
    Select,
}

/// The section a control belongs to, in both the panel and the written page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Picker,
    Pad_,
    Game,
    Saves,
    Picture,
    Capture,
    Link,
    Trainer,
    Agent,
    Battle,
    Portal,
    Music,
    Overlay,
}

impl Group {
    /// Heading for the written page.
    pub fn title(self) -> &'static str {
        match self {
            Group::Picker => "The ROM picker",
            Group::Pad_ => "The Game Boy itself",
            Group::Game => "Playing",
            Group::Saves => "Save states",
            Group::Picture => "How it looks",
            Group::Capture => "Capturing it",
            Group::Link => "The link cable",
            Group::Trainer => "Trainer view",
            Group::Agent => "The AI agent",
            Group::Battle => "Battle control",
            Group::Portal => "The trade portal",
            Group::Music => "Music mode",
            Group::Overlay => "Dialogs",
        }
    }

    /// One line under the heading, for a reader who arrived from a key.
    pub fn blurb(self) -> &'static str {
        match self {
            Group::Picker => {
                "The first screen, and where `L` takes you back to. Opens on the \
                 ROM you played last, so <kbd>Enter</kbd> resumes it."
            }
            Group::Pad_ => {
                "The four buttons are **configurable** (`key_a`, `key_b`, \
                 `key_start`, `key_select` in `config.toml`); the defaults are \
                 shown. A configured character always wins over an emulator \
                 control that wants the same key — `key_a = m` still means A, \
                 and mute moves out of the way."
            }
            Group::Game => "The emulator itself, while a game is running.",
            Group::Saves => {
                "Three slots per cartridge, held outside the game's own save so \
                 they cannot damage it."
            }
            Group::Picture => {
                "Everything here is cosmetic except `P`, which changes how the \
                 hardware is emulated."
            }
            Group::Capture => {
                "Two kinds of still and two kinds of moving picture — see the \
                 note under the table."
            }
            Group::Link => {
                "Bound **only when there is a cable**: a second cartridge \
                 (`link = <rom>`), a discovered local peer (`link = auto`, the \
                 default), or a socket (`link_net = …`). A key that does \
                 nothing is worse than no key at all."
            }
            Group::Trainer => {
                "Reads a running Pokémon Red/Blue cartridge. Read-only: it \
                 never writes to the game."
            }
            Group::Agent => {
                "An agent presses the buttons; it never writes to the game. \
                 Needs `plugins = agent` and a driver."
            }
            Group::Battle => {
                "Pokémon Red/Blue, `plugins = gen1-battle`. Writes the party \
                 into work RAM for the fight and puts it back afterwards; your \
                 `.sav` is untouched unless you save in-game."
            }
            Group::Portal => {
                "Asynchronous trades with friends over HTTPS. Needs \
                 `portal = <url>` and `plugins = gen1-link`."
            }
            Group::Music => {
                "A separate screen: `M` — from the picker **or from a running \
                 game** — or `terminal music` from the shell. It plays \
                 cartridges as albums and **never opens a save file**."
            }
            Group::Overlay => {
                "Keys that only exist while a dialog is up. Every one of them \
                 is escapable."
            }
        }
    }
}

/// One key, or one set of keys that do the same thing.
pub struct Control {
    /// How the key is written, as a reader would type it. Several spellings of
    /// one action are joined with ` · `.
    pub keys: &'static str,
    pub group: Group,
    /// One line. This is the middle column of the written page.
    pub action: &'static str,
    /// A caveat worth carrying wherever the key is listed — a condition, a
    /// limit, or a thing it deliberately does not do.
    pub note: Option<&'static str>,
    /// `(link text, path relative to `docs/`)` — the page that explains this
    /// key properly. `None` where there genuinely is nothing more to say.
    pub doc: Option<(&'static str, &'static str)>,
    pub slot: Slot,
    /// A substring that must appear in the rendered `?` overlay. This is the
    /// gate that stops the overlay and this registry drifting apart; `None`
    /// only for keys the overlay covers as a group.
    pub help: Option<&'static str>,
}

const fn c(
    keys: &'static str,
    group: Group,
    action: &'static str,
    doc: Option<(&'static str, &'static str)>,
    slot: Slot,
    help: Option<&'static str>,
) -> Control {
    Control {
        keys,
        group,
        action,
        note: None,
        doc,
        slot,
        help,
    }
}

/// Every key the terminal frontend binds.
///
/// Order is meaningful twice over: it is the order of the written page, and —
/// for entries with a panel [`Slot`] — the order of the in-app panel, which is
/// packed two-per-row within a group.
pub const CONTROLS: &[Control] = &[
    // ---- the picker -------------------------------------------------------
    Control {
        help: Some("↑/↓ j/k   move"),
        ..c(
            "↑ ↓ · j k",
            Group::Picker,
            "move the cursor",
            None,
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("enter   play"),
        ..c(
            "Enter",
            Group::Picker,
            "play the selected cartridge",
            Some(("Quick start", "quickstart.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some(
            "type to narrow the list; <kbd>Esc</kbd> clears it, \
             <kbd>Enter</kbd> plays the top match",
        ),
        help: Some("/         filter"),
        ..c(
            "/",
            Group::Picker,
            "filter by title",
            None,
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("r       rescan"),
        ..c(
            "r",
            Group::Picker,
            "rescan `rom_dir`",
            Some(("Where your ROMs go", "quickstart.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some(
            "works even when that cartridge lives outside `rom_dir`, so it is \
             not in the list",
        ),
        help: Some(".         resume last"),
        ..c(
            ".",
            Group::Picker,
            "resume the cartridge you played last",
            None,
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("g/G     top/bottom"),
        ..c(
            "g G · Home End",
            Group::Picker,
            "jump to the top or the bottom",
            None,
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some("the same key as in game — you do not have to come back here first"),
        help: Some("M         music mode"),
        ..c(
            "M",
            Group::Picker,
            "open music mode",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some("the same key as in game; do it before or after loading a cartridge"),
        help: Some("@  sign in"),
        ..c(
            "@",
            Group::Picker,
            "sign in to the trade portal",
            Some(("The trade portal", "portal.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("?         this help"),
        ..c(
            "?",
            Group::Picker,
            "the help overlay",
            None,
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("q/esc   quit"),
        ..c("q · Esc", Group::Picker, "quit", None, Slot::Hidden, None)
    },
    // ---- the game pad -----------------------------------------------------
    Control {
        help: Some("arrows    move"),
        ..c(
            "↑ ↓ ← →",
            Group::Pad_,
            "the d-pad",
            Some(("Joypad", "components/input.md")),
            Slot::Pad(Pad::Arrows),
            None,
        )
    },
    Control {
        help: Some("a/s     A / B"),
        ..c(
            "a",
            Group::Pad_,
            "**A** (`key_a`)",
            Some(("Joypad", "components/input.md")),
            Slot::Pad(Pad::A),
            None,
        )
    },
    c(
        "s",
        Group::Pad_,
        "**B** (`key_b`)",
        Some(("Joypad", "components/input.md")),
        Slot::Pad(Pad::B),
        None,
    ),
    Control {
        help: Some("x/z       Start/Sel"),
        ..c(
            "x",
            Group::Pad_,
            "**Start** (`key_start`)",
            Some(("Joypad", "components/input.md")),
            Slot::Pad(Pad::Start),
            None,
        )
    },
    c(
        "z",
        Group::Pad_,
        "**Select** (`key_select`)",
        Some(("Joypad", "components/input.md")),
        Slot::Pad(Pad::Select),
        None,
    ),
    // ---- playing ----------------------------------------------------------
    Control {
        help: Some("p       pause"),
        ..c(
            "p",
            Group::Game,
            "pause and resume",
            None,
            Slot::Half("p pause"),
            None,
        )
    },
    Control {
        help: Some("R         reset"),
        ..c(
            "R",
            Group::Game,
            "reset the machine",
            None,
            Slot::Half("R reset"),
            None,
        )
    },
    Control {
        note: Some(
            "0.5x · 1x · 2x · 4x · MAX. `1x` is the real 59.7275 Hz. Audio is \
             emitted at a fixed rate, so fast-forward **drops samples** rather \
             than raising the pitch",
        ),
        help: Some("+/-       speed"),
        ..c(
            "+ · = · - · _",
            Group::Game,
            "faster / slower",
            Some(("Fast-forward and audio", "audio.md")),
            Slot::Half("+/- speed"),
            None,
        )
    },
    Control {
        note: Some("it does not pause the game"),
        help: Some("t       stats overlay"),
        ..c(
            "t",
            Group::Game,
            "the stats overlay",
            Some(("Telemetry", "telemetry.md")),
            Slot::Half("t stats"),
            None,
        )
    },
    // Volume sits high in the panel on purpose: the panel is clipped from the
    // bottom on a short window, and an everyday control must not be the first
    // casualty. (A screenshot key can afford to be.)
    Control {
        note: Some(
            "**playback only** — the emulated machine's own master volume \
             (`NR50`) is untouched, so no test ROM, save state or capture can \
             see this",
        ),
        help: Some(", / .     volume down / up"),
        ..c(
            ", · < · . · >",
            Group::Game,
            "quieter / louder",
            Some(("Why this is not NR50", "audio.md")),
            Slot::Half(", . volume"),
            None,
        )
    },
    Control {
        note: Some("remembers the level you were at"),
        help: Some("mute (remembers the level)"),
        ..c(
            "m",
            Group::Game,
            "mute",
            Some(("Audio", "audio.md")),
            Slot::Half("m mute"),
            None,
        )
    },
    // `L` and `M` are a pair, and they are adjacent here so the panel packs
    // them onto one row. `L` used to be labelled "load ROM", which is what it
    // leads to rather than what it does, and hid that it is the way back to
    // the picker — the captain read the panel, could not find a route to the
    // shelf, and had to be told that `L` was it.
    Control {
        note: Some("music mode is `M`, from here or from the picker"),
        help: Some("L/M     picker / music"),
        ..c(
            "L",
            Group::Game,
            "back to the ROM picker",
            None,
            Slot::Half("L picker"),
            None,
        )
    },
    Control {
        note: Some(
            "puts the cartridge down first, exactly as <kbd>L</kbd> does; the \
             same key opens the shelf from the picker",
        ),
        help: Some("L/M     picker / music"),
        ..c(
            "M",
            Group::Game,
            "open music mode",
            Some(("Music mode", "music-mode.md")),
            Slot::Half("M music"),
            None,
        )
    },
    Control {
        note: Some("<kbd>Esc</kbd> leaves it again"),
        help: Some("f/esc     fullscreen on / off"),
        ..c(
            "f",
            Group::Game,
            "fullscreen — the picture, without the panels",
            Some(("Scaling and fit", "display.md")),
            Slot::Half("f full"),
            None,
        )
    },
    Control {
        note: Some("does nothing otherwise — it is not a second quit key"),
        help: Some("f/esc     fullscreen on / off"),
        ..c(
            "Esc",
            Group::Game,
            "leave fullscreen",
            Some(("Scaling and fit", "display.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("?         this help"),
        ..c(
            "?",
            Group::Game,
            "the help overlay",
            None,
            Slot::Half("? help"),
            None,
        )
    },
    Control {
        note: Some("asks first"),
        help: Some("q       quit"),
        ..c("q", Group::Game, "quit", None, Slot::Half("q quit"), None)
    },
    // ---- save states ------------------------------------------------------
    Control {
        note: Some(
            "there are three; <kbd>4</kbd>-<kbd>9</kbd> say so rather than doing \
             nothing, and a digit you have bound to a button stays that button",
        ),
        help: Some("1-3       save slot"),
        ..c(
            "1 2 3",
            Group::Saves,
            "choose the slot `F5`/`F8` will use",
            Some(("Save states", "savestates.md")),
            Slot::Wide("1-3 slot"),
            None,
        )
    },
    Control {
        note: Some(
            "a state is refused by a different cartridge, and by a build whose \
             state format has moved on",
        ),
        help: Some("F5/F8   save / load"),
        ..c(
            "F5",
            Group::Saves,
            "quick-save to the active slot",
            Some(("Save states", "savestates.md")),
            Slot::Half("F5 save"),
            None,
        )
    },
    c(
        "F8",
        Group::Saves,
        "quick-load the active slot",
        Some(("Save states", "savestates.md")),
        Slot::Half("F8 load"),
        None,
    ),
    // ---- how it looks -----------------------------------------------------
    Control {
        help: Some("[   palette"),
        ..c(
            "[",
            Group::Picture,
            "cycle the palette",
            Some(("Palettes and scaling", "display.md")),
            Slot::Half("[ palette"),
            None,
        )
    },
    Control {
        help: Some("]         upscaler"),
        ..c(
            "]",
            Group::Picture,
            "cycle the pixel-art upscaler",
            Some(("The upscalers", "upscaling.md")),
            Slot::Half("] upscale"),
            None,
        )
    },
    Control {
        note: Some("each algorithm allows its own factors, so this clamps"),
        help: Some("}       upscale factor"),
        ..c(
            "}",
            Group::Picture,
            "cycle the upscale factor",
            Some(("The upscalers", "upscaling.md")),
            Slot::Half("} factor"),
            None,
        )
    },
    // The fourth bracket key, and the one the family was missing. `protocol`
    // was config-file-only and needed a restart, which makes comparing two
    // transports in the same terminal a chore — and comparing them is the
    // whole reason anyone touches that key. See `App::cycle_protocol`.
    Control {
        note: Some(
            "kitty → sixel → iterm2 → half-blocks, the same four values \
             `protocol` takes in `config.toml`, without a restart. Forcing one \
             **turns detection off**, so a terminal that cannot draw the one \
             you picked shows nothing at all — the toast says when the terminal \
             never advertised it, and `{` again moves on",
        ),
        help: Some("{   protocol"),
        ..c(
            "{",
            Group::Picture,
            "cycle the graphics protocol",
            Some((
                "The four transports",
                "picture.md#how-to-find-out-which-one-you-got",
            )),
            Slot::Half("{ protocol"),
            None,
        )
    },
    Control {
        note: Some(
            "**this one is not cosmetic**: `identical` is the default and draws \
             the picture dot by dot as the hardware does; `standard` draws a \
             whole scanline at a time for about an eighth of the PPU cost. Most \
             games look the same either way",
        ),
        help: Some("P         picture: identical / standard"),
        ..c(
            "P",
            Group::Picture,
            "picture mode: `identical` ⇄ `standard`",
            Some(("Picture modes", "picture-modes.md")),
            Slot::Half("P picture mode"),
            None,
        )
    },
    // ---- capturing it -----------------------------------------------------
    Control {
        note: Some(
            "what you see: the palette and the upscaler are applied. \
             <kbd>F2</kbd> is the same key",
        ),
        help: Some("c         screenshot (or F2)"),
        ..c(
            "c · F2",
            Group::Capture,
            "screenshot",
            Some((
                "Screenshots",
                "picture.md#4-screenshots--c-and-c",
            )),
            Slot::Half("c shot"),
            None,
        )
    },
    Control {
        note: Some(
            "the archival one: exactly what the emulated hardware drew, \
             nothing applied",
        ),
        help: Some("C   screenshot (raw 160x144)"),
        ..c(
            "C",
            Group::Capture,
            "raw 160×144 screenshot",
            Some((
                "Why the raw one matters",
                "picture.md#4-screenshots--c-and-c",
            )),
            Slot::Half("C raw shot"),
            None,
        )
    },
    Control {
        note: Some(
            "starts **off**. The feed carries the **game picture only** — no \
             panels, by design; to stream the panels too, capture this window \
             in OBS",
        ),
        help: Some("v         live feed"),
        ..c(
            "v",
            Group::Capture,
            "live feed to `video_out` (a FIFO, or a webcam device) on / off",
            Some(("Video out and the virtual camera", "video-out.md")),
            Slot::Half("v stream"),
            None,
        )
    },
    Control {
        note: Some("a different job from `v`, hence a different key"),
        help: Some("V         record a timestamped file"),
        ..c(
            "V",
            Group::Capture,
            "record a timestamped file",
            Some(("Recording", "video-out.md")),
            Slot::Half("V record"),
            None,
        )
    },
    // ---- the link cable ---------------------------------------------------
    Control {
        note: Some(
            "with `link = <rom>` it swaps which of the two consoles takes your \
             keys; with a discovered or network peer it plugs the cable in and \
             out. The panel row says which",
        ),
        help: Some("W         swap which console"),
        ..c(
            "W",
            Group::Link,
            "swap console / cable on and off",
            Some(("The link cable", "link-cable.md")),
            Slot::Cable,
            None,
        )
    },
    // ---- trainer view -----------------------------------------------------
    Control {
        help: Some("T         show/hide"),
        ..c(
            "T",
            Group::Trainer,
            "show / hide the trainer panel",
            Some(("Trainer view", "trainer.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some("only bound while the panel is up, so it stays free as a game button"),
        help: Some("d       detail for the focused one"),
        ..c(
            "d",
            Group::Trainer,
            "detail for the focused Pokémon",
            Some(("Trainer view", "trainer.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some("in a battle the ring starts at the opponent, then your party"),
        help: Some("tab       next"),
        ..c(
            "Tab · Shift-Tab",
            Group::Trainer,
            "walk the focus ring",
            Some(("Trainer view", "trainer.md")),
            Slot::Hidden,
            None,
        )
    },
    // ---- the agent --------------------------------------------------------
    Control {
        help: Some("A         commentary panel"),
        ..c(
            "A",
            Group::Agent,
            "show / hide the commentary panel",
            Some(("Claude plays Pokémon", "claude-plays.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some("the game keeps running either way — this only stops something else pressing the buttons"),
        help: Some("G   agent on/off"),
        ..c(
            "G",
            Group::Agent,
            "hand the controller to the agent, or take it back",
            Some(("Claude plays Pokémon", "claude-plays.md")),
            Slot::Hidden,
            None,
        )
    },
    // ---- battle control ---------------------------------------------------
    Control {
        help: Some("B         start the selected battle"),
        ..c(
            "B",
            Group::Battle,
            "start the selected battle preset",
            Some(("Battle control", "gen1/battle-control.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("N   next preset"),
        ..c(
            "N",
            Group::Battle,
            "choose the next preset",
            Some(("Battle control", "gen1/battle-control.md")),
            Slot::Hidden,
            None,
        )
    },
    // ---- the trade portal -------------------------------------------------
    Control {
        note: Some("opens your browser; the device code is shown in the app too"),
        help: Some("@  sign in"),
        ..c(
            "@",
            Group::Portal,
            "sign in",
            Some(("The trade portal", "portal.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some(
            "picked from the party and boxes of the loaded save, **read out of \
             the cartridge, never typed**",
        ),
        help: Some("O  (in game) offer a Pokémon"),
        ..c(
            "O",
            Group::Portal,
            "offer a Pokémon",
            Some(("Offering from the emulator", "portal.md")),
            Slot::Hidden,
            None,
        )
    },
    // ---- music mode -------------------------------------------------------
    Control {
        help: Some("←/→ ↑/↓   browse"),
        ..c(
            "← → ↑ ↓ · h j k l",
            Group::Music,
            "browse the shelf — the music keeps playing",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("Home/End  first / last album"),
        ..c(
            "Home · End",
            Group::Music,
            "first / last album",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("⏎         play the selected album"),
        ..c(
            "Enter",
            Group::Music,
            "play the selected album",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("space     pause"),
        ..c(
            "Space",
            Group::Music,
            "pause",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some(
            "a cover is a *moment*, not just a picture — each candidate starts \
             the music where its own scene began",
        ),
        help: Some("c         next cover candidate"),
        ..c(
            "c",
            Group::Music,
            "next cover candidate",
            Some(("The cover", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("w         save what you are hearing as a WAV"),
        ..c(
            "w · W",
            Group::Music,
            "save what you are hearing as a WAV",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some("this time with a save file — music mode never opens one"),
        help: Some("g   play the game properly"),
        ..c(
            "g",
            Group::Music,
            "leave and play this cartridge properly",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("r   rescan"),
        ..c(
            "r",
            Group::Music,
            "rescan the shelf",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some(", / . m   volume / mute"),
        ..c(
            ", · . · m",
            Group::Music,
            "quieter / louder / mute",
            Some(("Audio", "audio.md")),
            Slot::Hidden,
            None,
        )
    },
    Control {
        help: Some("q / esc   back to the ROM picker"),
        ..c(
            "q · Q · Esc",
            Group::Music,
            "back to the ROM picker",
            Some(("Music mode", "music-mode.md")),
            Slot::Hidden,
            None,
        )
    },
    // ---- dialogs ----------------------------------------------------------
    Control {
        help: Some("press any key to close"),
        ..c(
            "any key",
            Group::Overlay,
            "close the `?` overlay",
            None,
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some("the prompt says so itself"),
        help: Some("y / enter confirms the quit prompt"),
        ..c(
            "y · Enter",
            Group::Overlay,
            "confirm the quit prompt — any other key cancels",
            None,
            Slot::Hidden,
            None,
        )
    },
    Control {
        note: Some(
            "type to filter, <kbd>Enter</kbd> arms the highlighted Pokémon and \
             <kbd>Enter</kbd> again submits it",
        ),
        help: Some("in the offer picker: ↑/↓ choose, type to filter"),
        ..c(
            "↑ ↓ · Enter · Esc",
            Group::Overlay,
            "the trade-offer picker",
            Some(("Offering from the emulator", "portal.md")),
            Slot::Hidden,
            None,
        )
    },
];

/// The four configurable game-pad characters, as the player has them.
#[derive(Clone, Copy)]
pub struct PadKeys {
    pub a: char,
    pub b: char,
    pub start: char,
    pub select: char,
}

impl PadKeys {
    fn label(&self, p: Pad) -> String {
        match p {
            Pad::Arrows => "Arrows  move".to_string(),
            Pad::A => format!("{}       A", self.a),
            Pad::B => format!("{}       B", self.b),
            Pad::Start => format!("{}       Start", self.start),
            Pad::Select => format!("{}       Select", self.select),
        }
    }
}

/// The in-app **Controls** panel, as plain lines.
///
/// `cable` is the label for `W`, or `None` when there is no cable — in which
/// case the key is not bound and the row is not drawn.
///
/// Pure: no ratatui, no session, so the packing is unit-testable.
pub fn panel_lines(pad: PadKeys, cable: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    for ctl in CONTROLS {
        if let Slot::Pad(p) = ctl.slot {
            out.push(pad.label(p));
        }
    }
    out.push(String::new());

    // Two per row, and a group never shares a row with the next one — the
    // grouping is the only thing making a seventeen-row list readable.
    let mut group: Option<Group> = None;
    let mut pending: Option<&str> = None;
    let flush = |pending: &mut Option<&str>, out: &mut Vec<String>| {
        if let Some(left) = pending.take() {
            out.push(left.to_string());
        }
    };
    for ctl in CONTROLS {
        let label = match ctl.slot {
            Slot::Half(l) | Slot::Wide(l) => l,
            Slot::Cable => match cable {
                Some(l) => l,
                None => continue,
            },
            _ => continue,
        };
        if group != Some(ctl.group) {
            flush(&mut pending, &mut out);
            group = Some(ctl.group);
        }
        match ctl.slot {
            Slot::Half(_) => match pending.take() {
                Some(left) => out.push(format!("{left:<12}{label}")),
                None => pending = Some(label),
            },
            _ => {
                flush(&mut pending, &mut out);
                out.push(label.to_string());
            }
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// The generated half of [`docs/controls.md`](../../../docs/controls.md).
///
/// Everything outside the `<!-- controls:begin -->` / `<!-- controls:end -->`
/// markers on that page is hand-written and is preserved; this is the table.
pub fn markdown() -> String {
    let mut out = String::new();
    let mut group: Option<Group> = None;
    for ctl in CONTROLS {
        if group != Some(ctl.group) {
            group = Some(ctl.group);
            out.push_str(&format!("\n### {}\n\n", ctl.group.title()));
            out.push_str(ctl.group.blurb());
            out.push_str("\n\n| Key | What it does | Read more |\n|---|---|---|\n");
        }
        let keys: Vec<String> = ctl
            .keys
            .split(" · ")
            .map(|k| format!("<kbd>{k}</kbd>"))
            .collect();
        let mut action = ctl.action.to_string();
        if let Some(n) = ctl.note {
            action.push_str(" — ");
            action.push_str(n);
        }
        let more = match ctl.doc {
            Some((text, path)) => format!("[{text}]({path})"),
            None => "—".to_string(),
        };
        out.push_str(&format!("| {} | {action} | {more} |\n", keys.join(" ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad() -> PadKeys {
        PadKeys {
            a: 'a',
            b: 's',
            start: 'x',
            select: 'z',
        }
    }

    /// The panel's job is to be a *reminder*, and a reminder that lists a key
    /// which does not exist is worse than none. Everything it shows comes from
    /// the registry, so the two cannot disagree.
    #[test]
    fn the_panel_is_packed_two_per_row_within_a_group() {
        let lines = panel_lines(pad(), None);
        // The pad block, then a blank, then the packed rows.
        assert_eq!(lines[0], "Arrows  move");
        assert_eq!(lines[5], "");
        let body = lines[6..].join("\n");
        // The pairs a player looks for, and the deliberate ordering: volume is
        // in the third packed row because the panel clips from the bottom.
        assert!(body.contains("p pause     R reset"), "{body}");
        assert_eq!(
            lines[8], ", . volume  m mute",
            "volume must stay high in the list — the panel is clipped from the \
             bottom and this is an everyday control\n{body}"
        );
        // A group never shares a row with the next one.
        assert!(body.contains("\n1-3 slot"), "{body}");
        assert!(body.contains("\n[ palette"), "{body}");
        assert!(body.contains("\nc shot"), "{body}");
    }

    /// A key that does nothing is worse than no key at all.
    #[test]
    fn the_cable_row_appears_only_when_there_is_a_cable() {
        assert!(!panel_lines(pad(), None).iter().any(|l| l.starts_with('W')));
        assert!(panel_lines(pad(), Some("W other console"))
            .iter()
            .any(|l| l == "W other console"));
    }

    /// The pad labels are the player's own characters, not ours.
    #[test]
    fn the_pad_block_uses_the_configured_keys() {
        let lines = panel_lines(
            PadKeys {
                a: 'j',
                b: 'k',
                start: 'n',
                select: 'b',
            },
            None,
        );
        assert_eq!(lines[1], "j       A");
        assert_eq!(lines[4], "b       Select");
    }

    /// Two entries for one key in one group would render two rows saying
    /// different things, which is exactly the drift this registry exists to
    /// stop. (`.`, `q`, `c`, `r`, `@` and `Enter` are each legitimately bound
    /// on more than one *screen*, so the check is per group.)
    #[test]
    fn no_key_is_registered_twice_in_one_group() {
        for (i, a) in CONTROLS.iter().enumerate() {
            for b in &CONTROLS[i + 1..] {
                assert!(
                    !(a.group == b.group && a.keys == b.keys),
                    "`{}` is registered twice under {}",
                    a.keys,
                    a.group.title()
                );
            }
        }
    }

    /// Groups are contiguous, because both the panel's row breaks and the
    /// page's headings are driven by "the group changed".
    #[test]
    fn every_group_is_contiguous() {
        let mut seen: Vec<Group> = Vec::new();
        let mut last: Option<Group> = None;
        for ctl in CONTROLS {
            if last != Some(ctl.group) {
                assert!(
                    !seen.contains(&ctl.group),
                    "{} is split into two runs",
                    ctl.group.title()
                );
                seen.push(ctl.group);
                last = Some(ctl.group);
            }
        }
    }

    #[test]
    fn the_markdown_has_a_row_for_every_control() {
        let md = markdown();
        assert_eq!(
            md.lines().filter(|l| l.starts_with("| <kbd>")).count(),
            CONTROLS.len()
        );
        // The links are the point of the page.
        assert!(md.contains("](picture-modes.md)"), "{md}");
        assert!(md.contains("](picture.md#4-screenshots--c-and-c)"), "{md}");
    }
}
