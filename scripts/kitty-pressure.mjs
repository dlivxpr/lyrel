#!/usr/bin/env node
// Deterministic terminal pressure fixture; run only inside an isolated lyrel pane.
// Placeholder marks are protocol test data from:
// https://raw.githubusercontent.com/kovidgoyal/kitty/master/gen/rowcolumn-diacritics.txt
import { deflateSync } from 'node:zlib';
import { base64Chunks, writeTerminal as emit } from './graphics-fixture-utils.mjs';

const options = { images: 24, width: 3840, height: 2160, path: 'mixed', mode: 'history', delay: 100 };
for (let i = 2; i < process.argv.length; i += 2) {
  const key = process.argv[i].replace(/^--/, '');
  if (!(key in options) || process.argv[i + 1] === undefined) throw new Error('Usage: node scripts/kitty-pressure.mjs --images 24 --width 3840 --height 2160 --path mixed --mode history --delay 100');
  options[key] = typeof options[key] === 'number' ? Number(process.argv[i + 1]) : process.argv[i + 1];
}
if (!process.stdout.isTTY) throw new Error('This fixture must write directly to a terminal, not a pipe or file');
for (const key of ['images', 'width', 'height', 'delay']) {
  if (!Number.isSafeInteger(options[key]) || options[key] < (key === 'delay' ? 0 : 1)) throw new Error(`Invalid --${key}`);
}
if (options.images > 4096 || options.width > 8192 || options.height > 8192 || options.width * options.height * 4 > 32 * 1024 * 1024) throw new Error('Fixture exceeds its bounded 32 MiB single-image allocation');
if (!['direct', 'placeholder', 'mixed'].includes(options.path) || !['history', 'visible'].includes(options.mode)) throw new Error('Invalid path or mode');

const marks = [0x305, 0x30d, 0x30e, 0x310, 0x312, 0x33d, 0x33e, 0x33f, 0x346, 0x34a, 0x34b, 0x34c, 0x350, 0x351, 0x352, 0x357].map(cp => String.fromCodePoint(cp));
const cols = Math.min(60, Math.max(2, (process.stdout.columns || 80) - 2));
const rows = 16;
const pixels = Buffer.alloc(options.width * options.height * 4);
for (let y = 0; y < options.height; y++) {
  for (let x = 0; x < options.width; x++) {
    const at = (y * options.width + x) * 4;
    pixels[at] = Math.floor(x * 255 / options.width);
    pixels[at + 1] = Math.floor(y * 255 / options.height);
    pixels[at + 2] = ((x >> 6) ^ (y >> 6)) & 1 ? 224 : 32;
    pixels[at + 3] = (x % 128 < 16 && y % 128 < 16) ? 128 : 255;
  }
}
const encoded = deflateSync(pixels).toString('base64');
await emit(`\r\nKITTY_PRESSURE_START ${JSON.stringify(options)} RGBA_BYTES=${pixels.length} ENCODED_BYTES=${Buffer.byteLength(encoded)}\r\n`);
for (let sequence = 0; sequence < options.images; sequence++) {
  const id = 1000 + sequence;
  const placeholder = options.path === 'placeholder' || (options.path === 'mixed' && sequence % 2 === 1);
  if (options.mode === 'visible') await emit('\x1b[3;1H');
  else await emit(`image ${sequence + 1}/${options.images} id=${id} path=${placeholder ? 'placeholder' : 'direct'}\r\n`);
  for (const { chunk, first, more } of base64Chunks(encoded)) {
    const header = first
      ? `a=T,i=${id},p=1,f=32,s=${options.width},v=${options.height},o=z,c=${cols},r=${rows},C=1,U=${placeholder ? 1 : 0},q=2,m=${more}`
      : `m=${more},q=2`;
    await emit(`\x1b_G${header};${chunk}\x1b\\`);
  }
  if (placeholder) {
    await emit(`\x1b[38;2;${id >>> 16};${(id >>> 8) & 255};${id & 255};58;5;1m`);
    for (let row = 0; row < rows; row++) {
      await emit(`\u{10eeee}${marks[row]}${marks[0]}${'\u{10eeee}'.repeat(cols - 1)}\x1b[0m\r\n`);
      if (row + 1 < rows) await emit(`\x1b[38;2;${id >>> 16};${(id >>> 8) & 255};${id & 255};58;5;1m`);
    }
  } else if (options.mode === 'history') await emit('\r\n'.repeat(rows));
  if (options.delay) await new Promise(resolve => setTimeout(resolve, options.delay));
}
await emit('\x1b[0m\r\nKITTY_PRESSURE_DONE: fixture exited; type ordinary shell input, revisit history, delete/clear, and record native process/GPU peaks separately.\r\n');
