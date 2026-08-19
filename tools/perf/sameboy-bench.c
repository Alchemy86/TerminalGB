/* Core-to-core throughput bench for SameBoy, so "we are faster" is a
 * measurement rather than a comparison of our number against their marketing.
 *
 * SameBoy ships its own `Tester`, but that is a harness: it handles buttons,
 * watches for stuck games and takes a screenshot, and it sets
 * `disable_rendering` — so timing it answers a different question from timing
 * our core. This is the same loop `examples/core_bench.rs` runs: N frames, the
 * thread CPU clock, nothing else.
 *
 *   tools/sameboy-oracle/fetch.sh          # builds SameBoy at the pinned commit
 *   D=target/sameboy-oracle
 *   cc -O2 -std=gnu99 -I$D/SameBoy -o /tmp/sbbench tools/perf/sameboy-bench.c \
 *      $D/SameBoy/build/obj/Core/*.o -lm
 *   taskset -c <a P-core> /tmp/sbbench blue.gb 7168 0 \
 *      $D/SameBoy/build/bin/BootROMs/dmg_boot.bin
 *
 * SameBoy is Expat-licensed and is built here at the commit
 * `tools/sameboy-oracle/fetch.sh` pins, with its own `CONF=release`
 * (`-O3 -ffast-math -DNDEBUG` plus LTO) — the comparison is only fair if both
 * sides are optimised, and that is the flag that says so.
 *
 * Nothing of SameBoy's is copied here; this file only calls its public API.
 * See docs/measured/against-the-field.md.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <Core/gb.h>

static void vblank(GB_gameboy_t *gb, GB_vblank_type_t t) { (void)gb; (void)t; }
static uint32_t rgb(GB_gameboy_t *gb, uint8_t r, uint8_t g, uint8_t b) {
    (void)gb; return (r<<16)|(g<<8)|b;
}
static void logcb(GB_gameboy_t *gb, const char *s, GB_log_attributes_t a) {
    (void)gb; (void)s; (void)a;
}

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "sbbench <rom> <frames> <render 0|1> [bootrom]\n"); return 2; }
    long frames = atol(argv[2]);
    int render = atoi(argv[3]);
    static GB_gameboy_t gb;
    GB_init(&gb, GB_MODEL_DMG_B);
    GB_set_vblank_callback(&gb, vblank);
    GB_set_rgb_encode_callback(&gb, rgb);
    GB_set_log_callback(&gb, logcb);
    if (argc > 4 && GB_load_boot_rom(&gb, argv[4])) { fprintf(stderr, "boot rom?\n"); return 1; }
    static uint32_t fb[160*144];
    GB_set_pixels_output(&gb, fb);
    if (GB_load_rom(&gb, argv[1])) { fprintf(stderr, "rom?\n"); return 1; }
    GB_set_turbo_mode(&gb, true, true);
    GB_set_rendering_disabled(&gb, !render);
    struct timespec a, b;
    clock_gettime(CLOCK_THREAD_CPUTIME_ID, &a);
    for (long i = 0; i < frames; i++) GB_run_frame(&gb);
    clock_gettime(CLOCK_THREAD_CPUTIME_ID, &b);
    double s = (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9;
    printf("%.3f s cpu for %ld frames = %.4f ms/frame = %.0fx realtime (render %d)\n",
           s, frames, s * 1000.0 / frames, frames / 59.7275 / s, render);
    return 0;
}
