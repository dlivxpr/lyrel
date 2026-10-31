#!/usr/bin/env node
// Owned native fixture; run only in an isolated lyrel pane.
// https://sw.kovidgoyal.net/kitty/graphics-protocol/ (2026-10-31).
import { once } from 'node:events';
import { createInterface } from 'node:readline';

if (!process.stdout.isTTY || !process.stdin.isTTY) {
  throw new Error('Run node scripts/kitty-playback.mjs inside an isolated native lyrel pane');
}
const id = 139141;
const width = 96;
const height = 64;
let uploads = 0;
let bytes = 0;
async function emit(text) {
  bytes += Buffer.byteLength(text);
  if (!process.stdout.write(text)) await once(process.stdout, 'drain');
}
async function control(keys) { await emit(`\x1b_Ga=a,i=${id},q=2,${keys}\x1b\\`); }
async function frame(color, gap = 500) {
  const pixels = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const edge = x < 4 || y < 4 || x >= width - 4 || y >= height - 4;
      pixels.set(edge ? [255, 255, 255, 255] : [...color, 255], 4 * (y * width + x));
    }
  }
  const encoded = pixels.toString('base64');
  for (let offset = 0; offset < encoded.length; offset += 4096) {
    const chunk = encoded.slice(offset, offset + 4096);
    const more = offset + chunk.length < encoded.length ? 1 : 0;
    const header = offset === 0
      ? `a=${uploads === 0 ? 't' : 'f'},i=${id},f=32,s=${width},v=${height},z=${gap},q=2,m=${more}`
      : `${uploads === 0 ? '' : 'a=f,'}q=2,m=${more}`;
    await emit(`\x1b_G${header};${chunk}\x1b\\`);
  }
  uploads++;
}
await emit('\x1b[2J\x1b[HKitty daemon playback: both white-bordered pictures must stay synchronized.\r\n');
await frame([220, 30, 40]);
await frame([30, 190, 60]);
await frame([30, 80, 230]);
await emit(`\x1b[5;3H\x1b_Ga=p,i=${id},p=1,c=18,r=6,C=1,q=2\x1b\\\x1b[5;25H\x1b_Ga=p,i=${id},p=2,c=18,r=6,C=1,q=2\x1b\\`);
await control('r=1,z=500');
await control('s=3,v=1');
await emit('\x1b[13;1Hplay / stop / loading / append / finite / gapless / 1 / 2 / 3 / stats / quit\r\n');
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of input) {
  const command = line.trim();
  if (command === 'play') await control('s=3,v=1');
  else if (command === 'stop') await control('s=1');
  else if (command === 'loading') await control('s=2,c=1');
  else if (command === 'finite') { await control('s=1,c=1'); await control('s=3,v=3'); }
  else if (command === 'gapless') await control('r=2,z=-1');
  else if (command === 'append') await frame([240, 240, 240]);
  else if (/^[123]$/.test(command)) await control(`s=1,c=${command}`);
  else if (command === 'quit') { input.close(); break; }
  else if (command !== 'stats') await emit('play / stop / loading / append / finite / gapless / 1 / 2 / 3 / stats / quit\r\n');
  await emit(`KITTY_PLAYBACK_STATE pid=${process.pid} uploads=${uploads} bytes=${bytes}\r\n`);
}
await emit(`KITTY_PLAYBACK_STOPPED pid=${process.pid} uploads=${uploads} bytes=${bytes}; playback remains daemon-owned after sender exits.\r\n`);
