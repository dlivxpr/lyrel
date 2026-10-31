#!/usr/bin/env node
// 只在自有隔离 lyrel 原生 pane 运行；不操作其他 pane/daemon。
// 发布规格：https://sw.kovidgoyal.net/kitty/graphics-protocol/（2026-10-31）。
import { once } from 'node:events';
import { createInterface } from 'node:readline';
if (!process.stdin.isTTY || !process.stdout.isTTY) throw new Error('Run inside an owned native lyrel pane');
const id = 139143;
const width = 96, height = 64;
let uploads = 0, bytes = 0, frames = [];
async function emit(text) {
  bytes += Buffer.byteLength(text);
  if (!process.stdout.write(text)) await once(process.stdout, 'drain');
}
async function command(keys) { await emit(`\x1b_Gi=${id},q=2,${keys}\x1b\\`); }
async function upload(name, color, { edit = 0, gap = 700 } = {}) {
  const w = edit ? 24 : width, h = edit ? 16 : height;
  const pixels = Buffer.alloc(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const edge = !edit && (x < 3 || y < 3 || x >= w - 3 || y >= h - 3);
    pixels.set(edge ? [255,255,255,255] : color, (y * w + x) * 4);
  }
  const action = frames.length ? 'f' : 't';
  const encoded = pixels.toString('base64');
  for (let pos = 0; pos < encoded.length; pos += 4096) {
    const chunk = encoded.slice(pos, pos + 4096), more = +(pos + chunk.length < encoded.length);
    const header = pos ? `${action === 'f' ? 'a=f,' : ''}m=${more},q=2`
      : `a=${action},i=${id},s=${w},v=${h},f=32,m=${more},q=2,${edit ? `r=${edit},x=48,y=32,X=1` : `z=${gap},X=1`}`;
    await emit(`\x1b_G${header};${chunk}\x1b\\`);
  }
  uploads++;
  if (!edit) frames.push(name);
}
async function reset() {
  await command('a=d,d=I'); frames = [];
  await upload('red', [220,30,40,255]);
  await upload('green', [30,190,60,255]);
  await upload('blue', [30,80,230,255]);
  await upload('source', [250,220,20,128], { gap:-1 });
  await emit(`\x1b[5;3H\x1b_Ga=p,i=${id},p=1,c=18,r=6,C=1,q=2\x1b\\\x1b[5;25H\x1b_Ga=p,i=${id},p=2,c=18,r=6,C=1,q=2\x1b\\`);
  await command('a=a,r=1,z=700'); await command('a=a,s=3,v=1,c=1');
}
async function remove(number, upper = false) {
  if (!frames.length) return;
  await command(`a=d,d=${upper ? 'F' : 'f'},r=${number}`);
  if (frames.length > 1 || upper) frames.splice(Math.min(frames.length, Math.max(1, number)) - 1, 1);
}
await emit('\x1b[2J\x1b[H#143: two placements stay synchronized; white border and untouched regions stay fixed.\r\n');
await reset();
await emit('\x1b[13;1Hreset / compose / replace / edit / current-delete / delete-first / delete-last / delete-only / append / loading / play / stop / stats / quit\r\n');
const input = createInterface({ input:process.stdin, crlfDelay:Infinity });
for await (const line of input) {
  const op = line.trim();
  const source = frames.indexOf('source') + 1, green = frames.indexOf('green') + 1, blue = frames.indexOf('blue') + 1;
  if (op === 'reset') await reset();
  else if (op === 'compose' && source && green) {
    await command(`a=c,r=${source},c=${green},X=8,Y=8,w=24,h=16,x=24,y=16`);
    await emit('COMPOSE_WHILE_PLAYING: green phase now has a blended yellow rectangle at (24,16); no upload.\r\n');
  } else if (op === 'replace' && source && blue) {
    await command(`a=c,r=${source},c=${blue},X=8,Y=8,w=24,h=16,x=24,y=16,C=1`);
    await emit('REPLACE_WHILE_PLAYING: blue phase now has a translucent yellow rectangle, not blended opaque pixels.\r\n');
  } else if (op === 'edit' && green) await upload('edit', [240,20,200,128], { edit:green });
  else if (op === 'current-delete' && green) {
    await command(`a=a,s=3,c=${green}`); await remove(green);
    await emit('DELETE_CURRENT_WHILE_PLAYING: green frame removed; blue succeeds it, remaining frame numbering is dense.\r\n');
  } else if (op === 'delete-first') await remove(1);
  else if (op === 'delete-last') await remove(frames.length);
  else if (op === 'delete-only' && frames.length === 1) await remove(1, true);
  else if (op === 'append') await upload('white', [240,240,240,255]);
  else if (op === 'loading') await command('a=a,s=2,c=1');
  else if (op === 'play') await command('a=a,s=3,v=1');
  else if (op === 'stop') await command('a=a,s=1');
  else if (op === 'quit') { input.close(); break; }
  else if (op !== 'stats') await emit('Command unavailable for the retained frames; use reset to restore the four-frame scene.\r\n');
  await emit(`KITTY_MUTATIONS pid=${process.pid} uploads=${uploads} bytes=${bytes} frames=${frames.join(',')}\r\n`);
}
await emit(`KITTY_MUTATIONS_STOPPED pid=${process.pid} uploads=${uploads} bytes=${bytes}; close/reopen the owned GUI without restarting this sender.\r\n`);
