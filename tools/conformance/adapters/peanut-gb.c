/* gbconform/1 adapter for **Peanut-GB** (https://github.com/deltabeard/Peanut-GB)
 * — a real, unrelated emulator, used to prove the contract's SERIAL half is
 * not secretly shaped around our own.
 *
 * Why this file exists
 * --------------------
 * binjgb proved the screen half (see `binjgb.sh`) and cannot prove the serial
 * half: its native API publishes no serial hook. Peanut-GB can, because
 * `gb_init_serial()` is part of its public API and its own test harness
 * already uses it. Nothing here patches or forks the emulator: this is a
 * frontend written against its published header, which is exactly what an
 * emulator with no headless mode has to write anyway.
 *
 * It is also an honest measure of what adoption costs in the worst case — the
 * "you have neither capability" row in docs/conformance-action.md §7. Peanut-GB
 * has no headless runner, no screenshot flag and no serial dump; supplying all
 * three took the ~100 lines below, most of them boilerplate the emulator's own
 * README already shows.
 *
 * Build (Peanut-GB is a single header, MIT, with no dependencies — we fetch it,
 * we do not vendor it):
 *
 *   git clone --depth 1 https://github.com/deltabeard/Peanut-GB /tmp/peanut
 *   cc -O2 -I/tmp/peanut -o /tmp/peanut-gbconform \
 *       tools/conformance/adapters/peanut-gb.c
 *
 * Run:
 *
 *   python3 tools/conformance/gbconform.py --adapter /tmp/peanut-gbconform \
 *       --out /tmp/peanut-out
 *
 * KNOWN, AND NOT A BUG IN THE CONTRACT: Peanut-GB is a DMG emulator. It does
 * not implement the Color console, so it ignores GBCONFORM_MODEL and any
 * CGB-pinned check will report whatever a DMG makes of that cartridge. That is
 * a true statement about the emulator, which is what the runner is for.
 */

#define ENABLE_SOUND 0
#define ENABLE_LCD 1
#include "peanut_gb.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct priv {
	uint8_t *rom;
	size_t rom_size;
	uint8_t *ram;
	size_t ram_size;
	FILE *serial;
	uint8_t frame[LCD_HEIGHT][LCD_WIDTH];
};

static uint8_t rom_read(struct gb_s *gb, const uint_fast32_t addr)
{
	const struct priv *p = gb->direct.priv;
	return addr < p->rom_size ? p->rom[addr] : 0xFF;
}

static uint8_t ram_read(struct gb_s *gb, const uint_fast32_t addr)
{
	const struct priv *p = gb->direct.priv;
	return addr < p->ram_size ? p->ram[addr] : 0xFF;
}

static void ram_write(struct gb_s *gb, const uint_fast32_t addr, const uint8_t v)
{
	struct priv *p = gb->direct.priv;
	if (addr < p->ram_size)
		p->ram[addr] = v;
}

/* The contract asks for the raw bytes the ROM sends to SB — no framing, no
 * interpretation. Whoever reads them decides what they mean. */
static void serial_tx(struct gb_s *gb, const uint8_t tx)
{
	struct priv *p = gb->direct.priv;
	if (p->serial)
		fputc(tx, p->serial);
}

static enum gb_serial_rx_ret_e serial_rx(struct gb_s *gb, uint8_t *rx)
{
	(void)gb; (void)rx;
	return GB_SERIAL_RX_NO_CONNECTION;   /* nothing on the other end */
}

static void draw_line(struct gb_s *gb, const uint8_t *pixels,
		      const uint_fast8_t line)
{
	struct priv *p = gb->direct.priv;
	if (line < LCD_HEIGHT)
		for (int x = 0; x < LCD_WIDTH; x++)
			p->frame[line][x] = pixels[x] & LCD_COLOUR;
}

static void on_error(struct gb_s *gb, const enum gb_error_e err,
		     const uint16_t val)
{
	(void)gb;
	/* An unimplemented opcode or a bad mapper is a REAL failure of the run,
	 * so exit non-zero: the runner must record an error, not a clean
	 * decline. Declining is "exit 0 having written nothing". */
	fprintf(stderr, "peanut-gb error %d at %04x\n", (int)err, val);
	exit(3);
}

static uint8_t *slurp(const char *path, size_t *size)
{
	FILE *f = fopen(path, "rb");
	if (!f)
		return NULL;
	fseek(f, 0, SEEK_END);
	long n = ftell(f);
	fseek(f, 0, SEEK_SET);
	uint8_t *buf = malloc(n > 0 ? (size_t)n : 1);
	if (buf && fread(buf, 1, (size_t)n, f) != (size_t)n) {
		free(buf);
		buf = NULL;
	}
	fclose(f);
	*size = (size_t)n;
	return buf;
}

/* Blargg's ROMs print their verdict and then loop forever; stopping there is
 * the difference between a two-second suite and a two-minute one.
 * (Hand-rolled rather than `memmem`, which is a GNU extension — an adapter
 * that needs a feature macro to compile is an adapter people do not adopt.) */
static int contains(const char *hay, size_t n, const char *needle)
{
	size_t m = strlen(needle);
	if (n < m)
		return 0;
	for (size_t i = 0; i + m <= n; i++)
		if (memcmp(hay + i, needle, m) == 0)
			return 1;
	return 0;
}

static int verdict_seen(const char *buf, size_t n)
{
	return contains(buf, n, "Passed") || contains(buf, n, "Failed");
}

int main(int argc, char **argv)
{
	const char *rom_path = getenv("GBCONFORM_ROM");
	if (!rom_path)
		rom_path = argc > 1 ? argv[1] : NULL;
	if (!rom_path) {
		fprintf(stderr, "usage: %s <rom.gb>  (or set GBCONFORM_ROM)\n", argv[0]);
		return 2;
	}
	const char *serial_out = getenv("GBCONFORM_SERIAL_OUT");
	const char *frame_out = getenv("GBCONFORM_FRAME_OUT");
	const char *frames_s = getenv("GBCONFORM_FRAMES");
	long frames = frames_s ? strtol(frames_s, NULL, 10) : 600;

	static struct priv priv;
	priv.rom = slurp(rom_path, &priv.rom_size);
	if (!priv.rom) {
		fprintf(stderr, "cannot read %s\n", rom_path);
		return 2;
	}

	struct gb_s gb;
	if (gb_init(&gb, rom_read, ram_read, ram_write, on_error, &priv) != GB_INIT_NO_ERROR) {
		fprintf(stderr, "peanut-gb declined this cartridge\n");
		return 2;
	}
	/* Cartridge RAM lives in memory only: a conformance run must never leave
	 * a .sav beside somebody's ROM. */
	if (gb_get_save_size_s(&gb, &priv.ram_size) != 0 || priv.ram_size == 0)
		priv.ram_size = 0x8000;      /* also covers non-battery RAM */
	priv.ram = calloc(priv.ram_size, 1);

	if (frame_out)
		gb_init_lcd(&gb, draw_line);
	if (serial_out) {
		priv.serial = fopen(serial_out, "wb");
		if (!priv.serial) {
			fprintf(stderr, "cannot write %s\n", serial_out);
			return 2;
		}
		gb_init_serial(&gb, serial_tx, serial_rx);
	}

	char tail[64] = {0};
	size_t tail_n = 0;
	for (long i = 0; i < frames; i++) {
		gb_run_frame(&gb);
		if (!priv.serial)
			continue;
		/* Peek at what we have written so far, cheaply, to stop early. */
		fflush(priv.serial);
		long here = ftell(priv.serial);
		if (here > 0) {
			long from = here > (long)sizeof(tail) ? here - (long)sizeof(tail) : 0;
			FILE *r = fopen(serial_out, "rb");
			if (r) {
				fseek(r, from, SEEK_SET);
				tail_n = fread(tail, 1, sizeof(tail), r);
				fclose(r);
			}
			if (verdict_seen(tail, tail_n))
				break;
		}
	}

	if (priv.serial)
		fclose(priv.serial);

	if (frame_out) {
		/* Netpbm P5: eight-bit greyscale, one byte per pixel. The runner
		 * compares shade RANK, so any four distinct greys will do — this
		 * emulator's shade 0 is white, so the ramp is inverted here. */
		FILE *f = fopen(frame_out, "wb");
		if (!f) {
			fprintf(stderr, "cannot write %s\n", frame_out);
			return 2;
		}
		fprintf(f, "P5\n%d %d\n255\n", LCD_WIDTH, LCD_HEIGHT);
		static const unsigned char shade[4] = {0xFF, 0xAA, 0x55, 0x00};
		for (int y = 0; y < LCD_HEIGHT; y++)
			for (int x = 0; x < LCD_WIDTH; x++)
				fputc(shade[priv.frame[y][x] & 3], f);
		fclose(f);
	}
	return 0;
}
