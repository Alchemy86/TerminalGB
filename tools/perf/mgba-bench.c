/* Core-to-core throughput bench for mGBA, the sibling of
 * `tools/perf/sameboy-bench.c` and for the same reason: "we are faster than X"
 * has to be a measurement on one box in one session, not our number set beside
 * a figure from somebody's README.
 *
 * mGBA is MPL-2.0 — the same licence as this repository — and is this
 * project's approved reference emulator (see `docs/measured/README.md`).
 * Nothing of it is copied here; this file only calls its public `mCore` API.
 *
 *   tools/blackout/fetch-and-build.sh     # builds mGBA at the pinned commit
 *   S=target/mgba/src B=target/mgba/build
 *   DEFS=(); while read -r d; do DEFS+=("$d"); done < <(
 *     grep -m1 C_DEFINES $B/CMakeFiles/mgba.dir/flags.make | tr ' ' '\n' | grep '^-D')
 *   cc -O2 -I$S/include -I$B/include "${DEFS[@]}" -o /tmp/mgbabench \
 *      tools/perf/mgba-bench.c $B/libmgba.a -lm -lpthread
 *   taskset -c 4 /tmp/mgbabench blue.gb 7168 0
 *
 * **Take the defines from mGBA's own `flags.make` and pass them as separate
 * words** — `tools/blackout/fetch-and-build.sh` documents why at length; the
 * short version is that getting it wrong segfaults rather than failing to
 * build, and this shell does not word-split an unquoted "$VAR".
 *
 * RENDER OFF is expressed by NOT calling `setVideoBuffer`. mGBA installs a
 * dummy renderer in `_GBCoreInit` (`src/gb/core.c:141`) and only swaps the
 * software one in on reset `if (gbcore->renderer.outputBuffer)`
 * (`src/gb/core.c:571`), which `setVideoBuffer` is what sets. So this is
 * mGBA's own supported no-rendering path, not a poke into its internals, and
 * it is the fair counterpart to SameBoy's `GB_set_rendering_disabled` and to
 * our own `--no-pixels`.
 *
 * Audio: mGBA's GB core emulates the APU regardless; `setAudioBufferSize`
 * only sizes the sink. A machine with no sound chip is one a cartridge can
 * tell apart (see `docs/measured/host.md`), so leaving it emulated is what
 * makes this the same machine our own collector runs.
 */
#include <mgba/core/config.h>
#include <mgba/core/core.h>

#include <stdio.h>
#include <stdlib.h>
#include <time.h>

int main(int argc, char** argv) {
	if (argc < 3) {
		fprintf(stderr, "mgbabench <rom> <frames> [render 0|1]\n");
		return 2;
	}
	const char* rom = argv[1];
	long frames = atol(argv[2]);
	int render = argc > 3 ? atoi(argv[3]) : 0;

	struct mCore* core = mCoreFind(rom);
	if (!core) {
		fprintf(stderr, "error: mGBA does not recognise %s\n", rom);
		return 1;
	}
	core->init(core);
	/* mGBA's cores read their options out of a config table on reset, so it
	 * has to exist before the first one. */
	mCoreInitConfig(core, NULL);

	mColor* video = NULL;
	if (render) {
		unsigned w, h;
		core->baseVideoSize(core, &w, &h);
		video = calloc((size_t) w * h, sizeof(mColor));
		core->setVideoBuffer(core, video, w);
	}
	core->setAudioBufferSize(core, 1024);

	if (!mCoreLoadFile(core, rom)) {
		fprintf(stderr, "error: mGBA could not load %s\n", rom);
		return 1;
	}
	core->reset(core);

	struct timespec a, b;
	clock_gettime(CLOCK_THREAD_CPUTIME_ID, &a);
	for (long i = 0; i < frames; i++) {
		core->runFrame(core);
	}
	clock_gettime(CLOCK_THREAD_CPUTIME_ID, &b);
	double s = (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9;

	printf("%.3f s cpu for %ld frames = %.4f ms/frame = %.0fx realtime (render %d)\n", s, frames,
	       s * 1000.0 / frames, frames / 59.7275 / s, render);

	mCoreConfigDeinit(&core->config);
	core->deinit(core);
	free(video);
	return 0;
}
