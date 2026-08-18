/* A second opinion, from somebody else's emulator.
 *
 * "SameBoy fails that one too" is the commonest excuse in this repository's
 * red rows, and until this existed it was always a quotation — usually of
 * SameSuite's `apu/README.md`, which is years out of date. This turns it into
 * a measurement: it runs a ROM on SameBoy's Core and prints the *ROM's own*
 * verdict, on whichever console you name.
 *
 * It reads exactly the two verdicts our harnesses read, and nothing else:
 *
 *   samesuite  the RESULT_CODE byte at $CFFE ('P' / 'F'), which
 *              `examples/samesuite.rs` reads
 *   blargg     the cartridge-RAM status byte at $A000 behind the
 *              $DE $B0 $61 signature, plus the report text from $A004,
 *              which `testharness/testroms.rs` reads
 *
 * plus one thing that is not a verdict at all:
 *
 *   peek       the bytes at the addresses named on the command line, after
 *              `frames` frames. A boot ROM is part of the silicon and this
 *              emulator does not run one, so "what does a real CGB leave in
 *              DIV for a CGB cartridge?" is a question only an implementation
 *              that runs the real boot ROM can answer.
 *
 * Nothing here is a hash of anybody's framebuffer, and nothing here is an
 * expectation of ours — the point is to ask a foreign implementation the same
 * question the ROM asks, so a disagreement names a real defect in one of us.
 *
 * Build with tools/sameboy-oracle/fetch.sh; see the README beside it.
 *
 *   oracle <bootrom-dir> <model> <verdict> <rom> [frames]
 *
 * SameBoy is Expat-licensed (except its iOS/ and HexFiend/ directories). This
 * file is ours and only calls its public Core API.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <Core/gb.h>

static char *no_input(GB_gameboy_t *gb) { return NULL; }
static void no_vblank(GB_gameboy_t *gb, GB_vblank_type_t kind) { }

/* SameBoy renders into this even with rendering disabled. */
static uint32_t pixels[256 * 224];

struct console {
    const char *name;
    GB_model_t model;
    const char *boot;
};

/* The names are the ones this repository already uses: `GB_MODEL` for a
 * console, `GB_CGB_REVISION` for a CPU CGB stepping. */
static const struct console CONSOLES[] = {
    { "dmg",  GB_MODEL_DMG_B, "dmg_boot.bin"  },
    { "mgb",  GB_MODEL_MGB,   "dmg_boot.bin"  },
    { "sgb",  GB_MODEL_SGB,   "sgb_boot.bin"  },
    { "cgb0", GB_MODEL_CGB_0, "cgb_boot.bin"  },
    { "cgbB", GB_MODEL_CGB_B, "cgb_boot.bin"  },
    { "cgbC", GB_MODEL_CGB_C, "cgb_boot.bin"  },
    { "cgbD", GB_MODEL_CGB_D, "cgb_boot.bin"  },
    { "cgbE", GB_MODEL_CGB_E, "cgb_boot.bin"  },
    { "agb",  GB_MODEL_AGB_A, "agb_boot.bin"  },
};

int main(int argc, char **argv)
{
    if (argc < 5) {
        fprintf(stderr,
                "usage: %s <bootrom-dir> <model> samesuite|blargg|peek <rom> [frames] [addr...]\n"
                "  model: dmg mgb sgb cgb0 cgbB cgbC cgbD cgbE agb\n",
                argv[0]);
        return 2;
    }
    const char *bootdir = argv[1], *model = argv[2], *what = argv[3], *rom = argv[4];
    int frames = argc > 5 ? atoi(argv[5]) : 3600;

    const struct console *c = NULL;
    for (size_t i = 0; i < sizeof CONSOLES / sizeof *CONSOLES; i++) {
        if (!strcmp(model, CONSOLES[i].name)) c = &CONSOLES[i];
    }
    if (!c) { fprintf(stderr, "unknown model '%s'\n", model); return 2; }

    GB_gameboy_t gb;
    GB_init(&gb, c->model);

    char path[4096];
    snprintf(path, sizeof path, "%s/%s", bootdir, c->boot);
    if (GB_load_boot_rom(&gb, path)) {
        fprintf(stderr, "cannot read boot ROM %s\n", path);
        return 2;
    }
    if (GB_load_rom(&gb, rom)) {
        fprintf(stderr, "cannot read ROM %s\n", rom);
        return 2;
    }
    GB_set_pixels_output(&gb, pixels);
    GB_set_vblank_callback(&gb, no_vblank);
    GB_set_async_input_callback(&gb, no_input);
    GB_set_rendering_disabled(&gb, true);

    int rc = 1;
    if (!strcmp(what, "samesuite")) {
        /* Stop as soon as the ROM has judged itself, so a short run is cheap
         * and a long one is bounded. */
        uint8_t v = 0;
        for (int f = 0; f < frames; f++) {
            GB_run_frame(&gb);
            v = GB_safe_read_memory(&gb, 0xCFFE);
            if (v == 'P' || v == 'F') break;
        }
        printf("%c %s (%s)\n", (v == 'P' || v == 'F') ? v : '?', rom, model);
        rc = (v == 'P') ? 0 : 1;
    }
    else if (!strcmp(what, "blargg")) {
        for (int f = 0; f < frames; f++) GB_run_frame(&gb);
        uint8_t sig[3];
        for (int i = 0; i < 3; i++) sig[i] = GB_safe_read_memory(&gb, 0xA001 + i);
        if (sig[0] != 0xDE || sig[1] != 0xB0 || sig[2] != 0x61) {
            printf("? %s (%s): no cartridge-RAM verdict (signature %02x%02x%02x)\n",
                   rom, model, sig[0], sig[1], sig[2]);
            rc = 2;
        }
        else {
            uint8_t status = GB_safe_read_memory(&gb, 0xA000);
            printf("%s %s (%s) status=%02x \"",
                   status == 0 ? "P" : status == 0x80 ? "?" : "F", rom, model, status);
            for (int i = 0; i < 512; i++) {
                uint8_t ch = GB_safe_read_memory(&gb, 0xA004 + i);
                if (!ch) break;
                putchar(ch == '\n' ? ' ' : ch);
            }
            printf("\"\n");
            rc = status == 0 ? 0 : 1;
        }
    }
    /* `peek`: run for `frames` and print the bytes at the addresses that
     * follow, in hex. Not a verdict — it is how you ask a foreign
     * implementation what a *real boot ROM* leaves behind, which is the one
     * question this repository cannot answer from its own source. See the
     * README beside this file. */
    else if (!strcmp(what, "peek")) {
        for (int f = 0; f < frames; f++) GB_run_frame(&gb);
        printf("%s (%s)", rom, model);
        for (int i = 6; i < argc; i++) {
            unsigned a = (unsigned)strtoul(argv[i], NULL, 16);
            printf(" %04x=%02x", a, GB_safe_read_memory(&gb, (uint16_t)a));
        }
        printf("\n");
        rc = 0;
    }
    else {
        fprintf(stderr, "unknown verdict kind '%s'\n", what);
        rc = 2;
    }

    GB_free(&gb);
    return rc;
}
