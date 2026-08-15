import('./gameboy/index.js')
  .catch(console.error);

import { render, status } from './gameboy/index.js';

const playButton = document.querySelector('#play');
const file = document.querySelector('#file');

let rom = null;

file.addEventListener('change', function readFile() {
  loadFile(this.files[0]);
});

playButton.addEventListener('click', function readFile() {
  if (rom) {
    playButton.disabled = true;
    render(rom);
    // Show what the emulator and its audio path are actually doing. The
    // browser's autoplay policy can leave the AudioContext suspended, and a
    // silent emulator with no explanation is indistinguishable from a broken
    // one — the same reason the terminal frontend has a Status panel.
    const statusEl = document.querySelector('#status');
    if (statusEl) {
      statusEl.hidden = false;
      setInterval(() => { statusEl.textContent = status(); }, 1000);
    }
  }
});

async function loadFile(file) {
  const { size, name } = file;
  playButton.removeAttribute('disabled');
  playButton.textContent = `Play ${name}`;
  const arrayBuffer = await file.arrayBuffer();
  const u8View = new Uint8Array(arrayBuffer);
  rom = u8View;
}

// Size the emulator canvas to the largest integer multiple of the native
// 160x144 that fits the viewport, centred (CSS flex centres it; the surrounding
// #game backdrop is the black letterbox). Integer scaling keeps `pixelated`
// rendering crisp — every source pixel maps to the same NxN block. Clamps to 1x
// so a tiny window crops rather than collapsing the canvas to nothing.
const NATIVE_W = 160;
const NATIVE_H = 144;

function fitCanvas() {
  const canvas = document.querySelector('#game canvas');
  if (!canvas) return;
  const scale = Math.max(1, Math.min(
    Math.floor(window.innerWidth / NATIVE_W),
    Math.floor(window.innerHeight / NATIVE_H),
  ));
  canvas.style.width = `${NATIVE_W * scale}px`;
  canvas.style.height = `${NATIVE_H * scale}px`;
}

window.addEventListener('resize', fitCanvas);

// The wasm renderer creates the <canvas> lazily inside #game (only after Play),
// so watch for it and fit as soon as it appears, then on every resize.
const gameEl = document.querySelector('#game');
if (gameEl) {
  new MutationObserver(fitCanvas).observe(gameEl, { childList: true });
}
