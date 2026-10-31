#!/usr/bin/env node
// Owned native smoke fixture: run inside an isolated lyrel pane only.
// Protocol baseline: https://sw.kovidgoyal.net/kitty/graphics-protocol/ (2026-10-31).
import { once } from 'node:events';
import { createInterface } from 'node:readline';

if (!process.stdout.isTTY || !process.stdin.isTTY) {
  throw new Error('Run node scripts/kitty-frames.mjs inside an isolated native lyrel pane');
}
const width = 96;
const height = 64;
const id = 139140;
let uploads = 0;
let bytes = 0;
async function emit(text) {
  bytes += Buffer.byteLength(text);
  if (!process.stdout.write(text)) await once(process.stdout, 'drain');
}
async function upload(action, color) {
  const pixels = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const at = 4 * (y * width + x);
      const edge = x < 4 || y < 4 || x >= width - 4 || y >= height - 4;
      pixels.set(edge ? [255, 255, 255, 255] : [...color, 255], at);
    }
  }
  const encoded = pixels.toString('base64').replace(/=+$/, '');
  for (let offset = 0; offset < encoded.length; offset += 4096) {
    const chunk = encoded.slice(offset, offset + 4096);
    const more = offset + chunk.length < encoded.length ? 1 : 0;
    const header = offset === 0
      ? `a=${action},i=${id},f=0,s=${width},v=${height},q=2,m=${more}`
      : `${action === 'f' ? 'a=f,' : ''}q=2,m=${more}`;
    await emit(`\x1b_G${header};${chunk}\x1b\\`);
  }
  uploads++;
}
await emit('\x1b[2J\x1b[HKitty complete frames: red / green / blue; both placements must match.\r\n');
await upload('t', [220, 30, 40]);
await upload('f', [30, 190, 60]);
await upload('f', [30, 80, 230]);
await emit(`\x1b[5;3H\x1b_Ga=p,i=${id},p=1,c=18,r=6,C=1,q=2\x1b\\`);
await emit(`\x1b[5;25H\x1b_Ga=p,i=${id},p=2,c=18,r=6,C=1,q=2\x1b\\`);
await emit('\x1b[13;1HType 1, 2, or 3 + Enter to select a frame; stats, white, delete, or quit.\r\n');
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of input) {
  const command = line.trim();
  if (/^[123]$/.test(command)) {
    await emit(`\x1b_Ga=a,i=${id},s=1,c=${command},q=2\x1b\\`);
  } else if (command === 'white') {
    await upload('f', [240, 240, 240]);
    await emit(`\x1b_Ga=a,i=${id},s=1,c=${uploads},q=2\x1b\\`);
  } else if (command === 'delete') {
    await emit(`\x1b_Ga=d,d=I,i=${id},q=2\x1b\\`);
  } else if (command === 'quit') {
    input.close();
    break;
  } else if (command !== 'stats') {
    await emit('Commands: 1 / 2 / 3 / stats / white / delete / quit\r\n');
  }
  await emit(`KITTY_FRAMES_STATE pid=${process.pid} uploads=${uploads} bytes=${bytes}\r\n`);
}
await emit(`KITTY_FRAMES_STOPPED pid=${process.pid} uploads=${uploads} bytes=${bytes}; retained pictures require the same living daemon.\r\n`);
