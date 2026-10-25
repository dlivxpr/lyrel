import fs from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';

if (!process.stdin.isTTY || !process.env.LYREL_PANE) throw new Error('requires a real lyrel Linux PTY');
process.stdin.setRawMode(true);
process.stdin.resume();
let buffered = Buffer.alloc(0);
let awaken;
process.stdin.on('data', chunk => {
  buffered = Buffer.concat([buffered, chunk]);
  if (awaken) { const wake = awaken; awaken = undefined; wake(); }
});
async function inputReady(deadline) {
  if (Date.now() >= deadline) throw new Error('PTY input/reply deadline');
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { awaken = undefined; reject(new Error('PTY input/reply deadline')); }, deadline - Date.now());
    awaken = () => { clearTimeout(timer); resolve(); };
  });
}
async function takeUntil(ending) {
  const deadline = Date.now() + 90000;
  for (;;) {
    const index = buffered.indexOf(ending);
    if (index >= 0) {
      const result = buffered.subarray(0, index + ending.length);
      buffered = buffered.subarray(index + ending.length);
      return result;
    }
    await inputReady(deadline);
  }
}
async function reply() {
  // Kitty replies terminate in ST; geometry replies terminate in t.
  const deadline = Date.now() + 90000;
  while (buffered.length < 2) await inputReady(deadline);
  return takeUntil(buffered[1] === 0x5b ? Buffer.from('t') : Buffer.from('\x1b\\'));
}
async function out(bytes) {
  if (!process.stdout.write(bytes)) await new Promise(resolve => process.stdout.once('drain', resolve));
}
const stats = { bytes: 0, uploads: 0, placements: 0 };
function record(bytes) {
  stats.bytes += bytes.length;
  const text = bytes.toString('latin1');
  for (const match of text.matchAll(/\x1b_G([^;\x1b]*)[;\x1b]/g)) {
    const action = match[1].split(',').find(field => field.startsWith('a='));
    if (action === 'a=t' || action === 'a=T') stats.uploads++;
    if (action === 'a=p') stats.placements++;
  }
}
const barrier = Buffer.from('\x1b_Ga=q,i=4294967294,f=32,s=1,v=1;/wAA/w==\x1b\\');
const expected = Buffer.from('\x1b_Gi=4294967294;OK\x1b\\');
await out('\r\nKITTY_READY\r\n');
for (;;) {
  const line = (await takeUntil(Buffer.from('\n'))).toString().trim();
  const [operation, serial, countText, payload, fragmentText = '0', gateHex] = line.split(/\s+/);
  if (!/^(SEND|SEND_OK|WAIT|STAT|EXIT|FLOOD)$/.test(operation)) throw new Error(`invalid fixture operation: ${operation}`);
  if (operation === 'EXIT') break;
  const replies = Number(countText);
  const fragment = Number(fragmentText);
  if (!Number.isSafeInteger(replies) || replies < 0 || !Number.isSafeInteger(fragment) || fragment < 0 || !/^(?:[0-9a-f]{2})*$/i.test(payload)) throw new Error('invalid fixture request');
  const bytes = Buffer.from(payload, 'hex');
  let gate;
  if (operation === 'WAIT') {
    gate = Buffer.from(gateHex, 'hex').toString();
    // Only the private directory containing this producer can hold gates/logs.
    const root = new URL('.', import.meta.url).pathname;
    if (!gate.startsWith(root) || gate.slice(root.length).includes('/')) throw new Error('gate outside fixture directory');
    await out(`\r\nKITTY_WAITING_${serial}\r\n`);
    const deadline = Date.now() + 90000;
    while (!fs.existsSync(gate)) {
      if (Date.now() >= deadline) throw new Error('remote fixture gate deadline');
      await delay(2);
    }
  }
  if (operation === 'STAT') {
    await out(`\x1b7\x1b[2;1H\r\nKITTY_STATS_${serial} pid=${process.pid} bytes=${stats.bytes} uploads=${stats.uploads} placements=${stats.placements}\r\n\x1b8`);
  } else if (operation === 'FLOOD') {
    const begin = Buffer.from('\x1b_Ga=T,i=720,s=1,v=1;');
    record(begin);
    await out(begin);
    const block = Buffer.alloc(8192, 65);
    for (let i = 0; i < 49 * 1024 * 1024 / block.length; i++) await out(block);
    stats.bytes += 49 * 1024 * 1024;
    const end = Buffer.from('\x1b\\after_oversize');
    record(end);
    await out(end);
  } else {
    record(bytes);
    for (let start = 0; start < bytes.length; start += fragment || bytes.length) await out(bytes.subarray(start, start + (fragment || bytes.length)));
  }
  for (let i = 0; i < replies; i++) {
    const answer = await reply();
    if (operation === 'SEND_OK' && !answer.subarray(-5).equals(Buffer.from(';OK\x1b\\'))) throw new Error('graphics admission refused');
    await out(`\x1b7\x1b[1;1H\r\nKITTY_REPLY ${serial} ${answer.toString('hex')}\r\n\x1b8`);
  }
  await out(barrier);
  if (!(await reply()).equals(expected)) throw new Error('query barrier mismatch');
  await out(`\x1b7\x1b[1;1H\r\nKITTY_DONE_${serial}\r\n\x1b8`);
  if (gate) fs.writeFileSync(`${gate}.done`, '');
}
process.stdin.setRawMode(false);
process.stdin.pause();
