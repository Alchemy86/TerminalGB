// Reproduce Gen 1's blackout on mGBA, so the answer comes from somebody else's
// emulator as well as ours.
//
// Pokémon Red/Blue do not end a run when your last Pokémon faints:
// `HandleBlackOut` (`home/overworld.asm`) fades the screen out, stops the
// music, halves your money and warps you to `wLastBlackoutMap`. Before your
// first Pokémon Centre that is Pallet Town, so you wake up outside your own
// front door.
//
// `examples/blackout_probe.rs` shows that TerminalGB completes that sequence
// when the machine has a sound chip and hangs for ever when it does not,
// because `FadeOutAudio` (`home/fade_audio.asm`) walks the master volume down
// by reading `rAUDVOL` back and only stops when the read-back is zero. This
// asks mGBA the same question, from the same setup, so the expected answer is
// not one TerminalGB supplied.
//
// mGBA is MPL-2.0, the same licence as this repository. Nothing is vendored:
// it is fetched, built and linked at run time, exactly as
// `tools/sameboy-oracle/` and `tools/bgb-interop/` do for their references.
//
// Build and run (see README.md in this directory — do not hand-roll the
// compile line, it has two segfaulting traps in it):
//
//   tools/blackout/fetch-and-build.sh
//   target/mgba/mgba-blackout --rom <blue.gb> \
//       --party target/blackout-probe/party.bin \
//       --setup target/blackout-probe/setup.txt
//
// The two input files are written by `examples/blackout_probe.rs`. The party
// block is copied verbatim so both emulators fight with the same Pokémon; the
// opponent is armed through `wCurOpponent`, which is the cartridge's own
// mechanism and what its debug menu uses.

#include <mgba/core/config.h>
#include <mgba/core/core.h>
#include <mgba/internal/gb/input.h>

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// AtlasGB `pokemon-rb`, verified addresses.
#define W_IS_IN_BATTLE 0xD057
#define W_CUR_MAP 0xD35E
#define W_LAST_BLACKOUT_MAP 0xD719
#define W_AUDIO_FADE_OUT_CONTROL 0xCFC7
#define W_PLAY_TIME_MINUTES 0xDA43
#define W_PLAY_TIME_SECONDS 0xDA44

#define KEY(k) (1u << (k))

static struct mCore* core;

static void run(unsigned frames) {
	for (unsigned i = 0; i < frames; ++i) {
		core->runFrame(core);
	}
}

static void hold(int key, unsigned frames) {
	core->setKeys(core, KEY(key));
	run(frames);
	core->setKeys(core, 0);
}

// One press, then the gap Gen 1's own menu handler needs before it will see the
// next one: `HandleMenuInput` delays several frames after accepting a button.
static void press(int key) {
	hold(key, 4);
	run(24);
}

static uint8_t rd(uint32_t a) {
	return (uint8_t) core->busRead8(core, a);
}

// How many distinct greys are on screen. A Game Boy frame is four shades, so
// "one shade" is a categorically blank picture and needs no threshold.
static int distinct_shades(const mColor* buf, size_t n) {
	int seen[256];
	memset(seen, 0, sizeof seen);
	for (size_t i = 0; i < n; ++i) {
		seen[buf[i] & 0xFF] = 1;
	}
	int c = 0;
	for (int i = 0; i < 256; ++i) {
		c += seen[i];
	}
	return c;
}

static const char* arg(int argc, char** argv, const char* flag) {
	for (int i = 1; i + 1 < argc; ++i) {
		if (!strcmp(argv[i], flag)) {
			return argv[i + 1];
		}
	}
	return NULL;
}

// A `key=value` line out of the probe's `setup.txt`.
static long setup_value(const char* path, const char* key) {
	FILE* f = fopen(path, "r");
	if (!f) {
		fprintf(stderr, "error: cannot read %s\n", path);
		exit(1);
	}
	char line[256];
	long v = -1;
	size_t klen = strlen(key);
	while (fgets(line, sizeof line, f)) {
		if (!strncmp(line, key, klen) && line[klen] == '=') {
			v = strtol(line + klen + 1, NULL, 0);
			break;
		}
	}
	fclose(f);
	if (v < 0) {
		fprintf(stderr, "error: %s has no `%s=`\n", path, key);
		exit(1);
	}
	return v;
}

int main(int argc, char** argv) {
	const char* rom = arg(argc, argv, "--rom");
	const char* party_path = arg(argc, argv, "--party");
	const char* setup_path = arg(argc, argv, "--setup");
	const char* frames_s = arg(argc, argv, "--frames");
	unsigned observe = frames_s ? (unsigned) strtoul(frames_s, NULL, 10) : 3000;
	if (!rom || !party_path || !setup_path) {
		printf("error: --rom, --party and --setup are all required\n");
		printf("help: ./mgba-blackout --rom <blue.gb> "
		       "--party target/blackout-probe/party.bin "
		       "--setup target/blackout-probe/setup.txt\n");
		return 2;
	}

	long party_addr = setup_value(setup_path, "party_addr");
	long party_len = setup_value(setup_path, "party_len");
	long opp_addr = setup_value(setup_path, "cur_opponent_addr");
	long opp = setup_value(setup_path, "cur_opponent");
	long lvl_addr = setup_value(setup_path, "cur_enemy_level_addr");
	long lvl = setup_value(setup_path, "cur_enemy_level");

	uint8_t* party = malloc((size_t) party_len);
	FILE* pf = fopen(party_path, "rb");
	if (!pf || fread(party, 1, (size_t) party_len, pf) != (size_t) party_len) {
		fprintf(stderr, "error: cannot read %ld bytes from %s\n", party_len, party_path);
		return 1;
	}
	fclose(pf);

	core = mCoreFind(rom);
	if (!core) {
		fprintf(stderr, "error: mGBA does not recognise %s\n", rom);
		return 1;
	}
	core->init(core);
	// mGBA's cores read their options out of a config table on reset, so it
	// has to exist before the first one.
	mCoreInitConfig(core, NULL);
	unsigned w, h;
	core->baseVideoSize(core, &w, &h);
	mColor* video = calloc((size_t) w * h, sizeof(mColor));
	core->setVideoBuffer(core, video, w);
	// Nobody listens, but the chip is emulated — which is the whole point.
	core->setAudioBufferSize(core, 1024);
	if (!mCoreLoadFile(core, rom)) {
		fprintf(stderr, "error: mGBA could not load %s\n", rom);
		return 1;
	}
	core->reset(core);

	printf("mgba blackout probe\n");
	printf("  rom      %s\n", rom);
	printf("  party    %s (%ld bytes at $%04lX)\n", party_path, party_len, party_addr);
	printf("  opponent species %ld at level %ld\n", opp, lvl);
	printf("  observe  %u frames of no input after the faint\n\n", observe);

	// A brand-new game, exactly as the Rust probe starts one: mashing
	// START/A plays Gen 1's intro on its own, and the play clock moving is
	// what says the game is running rather than sitting on the title screen.
	int running = 0;
	for (int step = 0; step < 500; ++step) {
		hold(step % 2 == 0 ? GB_KEY_START : GB_KEY_A, 4);
		run(20);
		if (rd(W_PLAY_TIME_MINUTES) > 0 || rd(W_PLAY_TIME_SECONDS) > 4) {
			running = 1;
			break;
		}
	}
	if (!running) {
		printf("error: never reached the overworld from the title screen\n");
		return 1;
	}
	// B, never A: both dismiss a text box, but B also closes a menu and can
	// never open one.
	for (int i = 0; i < 8; ++i) {
		hold(GB_KEY_B, 4);
		run(20);
	}
	run(60);

	// The party the plugin built on our side, byte for byte.
	for (long i = 0; i < party_len; ++i) {
		core->busWrite8(core, (uint32_t) (party_addr + i), party[i]);
	}
	// ...and the opponent, through the cartridge's own mechanism:
	// `OverworldLoop` polls `wCurOpponent` and starts the battle itself.
	core->busWrite8(core, (uint32_t) lvl_addr, (uint8_t) lvl);
	core->busWrite8(core, (uint32_t) opp_addr, (uint8_t) opp);

	int started = 0;
	for (int i = 0; i < 600; ++i) {
		core->runFrame(core);
		if (rd(W_IS_IN_BATTLE) != 0) {
			started = 1;
			break;
		}
	}
	if (!started) {
		printf("error: the battle never started (wCurOpponent was not picked up)\n");
		return 1;
	}

	int fainted = 0;
	for (int i = 0; i < 900; ++i) {
		if (rd(W_IS_IN_BATTLE) == 0xFF) {
			fainted = 1;
			break;
		}
		press(GB_KEY_A);
	}
	if (!fainted) {
		printf("error: the party never fainted\n");
		return 1;
	}

	uint8_t map_at_faint = rd(W_CUR_MAP);
	uint8_t last_blackout = rd(W_LAST_BLACKOUT_MAP);
	printf("  at the faint            wCurMap %u ($D35E), wLastBlackoutMap %u ($D719)\n",
	       map_at_faint, last_blackout);

	// Nothing is pressed from here.
	core->setKeys(core, 0);
	int back_at = -1;
	int recovered = 0;
	unsigned seen = 0;
	for (unsigned f = 0; f < observe; ++f) {
		core->runFrame(core);
		seen = f + 1;
		int shades = distinct_shades(video, (size_t) w * h);
		if (shades > 1 && back_at < 0) {
			back_at = (int) f;
		}
		if (rd(W_IS_IN_BATTLE) == 0 && shades > 1) {
			recovered = 1;
			for (int i = 0; i < 120; ++i) {
				core->runFrame(core);
				++seen;
			}
			break;
		}
	}

	printf("  after %5u frames      wCurMap %u, wIsInBattle $%02X ($D057)\n",
	       seen, rd(W_CUR_MAP), rd(W_IS_IN_BATTLE));
	printf("  screen                  %d distinct shade(s)",
	       distinct_shades(video, (size_t) w * h));
	if (back_at >= 0) {
		printf(", came back at frame %d\n", back_at);
	} else {
		printf(", never came back\n");
	}
	printf("  wAudioFadeOutControl    $%02X ($CFC7)\n", rd(W_AUDIO_FADE_OUT_CONTROL));
	printf("\nverdict\n  mGBA %s\n",
	       recovered ? "COMPLETED the whiteout — the player woke up"
	                 : "did NOT complete the whiteout");

	core->unloadROM(core);
	mCoreConfigDeinit(&core->config);
	core->deinit(core);
	free(video);
	free(party);
	return recovered ? 0 : 1;
}
