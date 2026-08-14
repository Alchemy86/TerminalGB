"""GB Emulator Shootout adapter for TerminalGB.

Copy to `emulators/terminalgb.py` in a checkout of
https://github.com/gbdev/GBEmulatorShootout, add the `EMULATOR_SPECS` entry
from `main.py.snippet` beside it, and the workflow from `ci-terminalgb.yml`.
See `docs/shootout.md` in this repository for the whole submission.

Written against the shootout at commit 38b926b.
"""

from util import *
from emulator import Emulator
from test import *


class TerminalGB(Emulator):
    def __init__(self):
        super().__init__(
            "TerminalGB",
            "https://github.com/Alchemy86/TerminalGB",
            startup_time=1.0,
            # PCM12/PCM34 ($FF76/$FF77) are implemented, so the 57 SameSuite
            # APU ROMs that read them back are run rather than skipped. We pass
            # 55 of them.
            features=(PCM,),
        )
        # The *window title* is a published constant in the emulator
        # (`screen::desktop::WINDOW_TITLE`, pinned by a unit test); everything
        # the frontend appends to it — the upscaler's name, the volume — comes
        # after this prefix. The entry's display name above happens to match
        # today, but the two are separate decisions.
        self.title_check = lambda title: title.startswith("TerminalGB")
        # Matches `--speed 15` below: the runner's per-test time budget is
        # divided by this, so the two must move together.
        self.speed = 15.0

    def setup(self):
        downloadGithubRelease(
            "Alchemy86/TerminalGB",
            "downloads/terminalgb.zip",
            filter=lambda n: "win" in n and n.endswith(".zip"),
            require_asset=True,
        )
        extract("downloads/terminalgb.zip", "emu/terminalgb")
        setDPIScaling("emu/terminalgb/terminalgb.exe")
        # An OpenGL frontend on a runner with no GPU needs a software GL stack,
        # the same way gameroy does.
        setupMesa(r"emu\terminalgb")

    def startProcess(self, rom, *, model, required_features):
        gb_model = {DMG: "dmg", CGB: "cgb", SGB: "sgb"}.get(model)
        if gb_model is None:
            return None
        return subprocess.Popen(
            [
                "emu/terminalgb/terminalgb.exe",
                os.path.abspath(rom),
                # Which console to boot. Not inferred from the cartridge header
                # here: nine Mooneye tests print their boot registers, so a DMG
                # row run in a Color console's compatibility mode fails however
                # correct the CPU is.
                "--model", gb_model,
                # Client area of exactly 160x144 device pixels, non-resizable.
                "--scale", "1",
                # …showing the untouched frame, whatever a config file says.
                "--upscale", "none",
                "--upscale-factor", "1",
                # Turbo, so 264 test ROMs finish in a CI run.
                "--speed", "15",
                # The emulator pauses when its window loses focus, which is
                # right for a person and fatal for a runner that may never hand
                # it focus at all.
                "--run-unfocused",
            ],
            cwd="emu/terminalgb",
        )
