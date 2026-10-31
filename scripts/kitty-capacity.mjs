#!/usr/bin/env node
// Owned native workload; use only an isolated lyrel pane, never a user's session.
// Same direct Kitty APC/PTY path as kitty-playback.mjs; no PTY or renderer here.
import { createHash } from 'node:crypto';
import { once } from 'node:events';
import { readFile, writeFile } from 'node:fs/promises';

const usage = 'node scripts/kitty-capacity.mjs --pane 0|1 [--command load|start|stop|edit|compose|delete|replace|stats|select] [--frame 1..60] [--exit-after-start] [--manifest PATH]';
const options = { pane: null, command: null, frame: 2, exitAfterStart: false, manifest: null };
for (let index = 2; index < process.argv.length; index++) {
  const key = process.argv[index];
  if (key === '--help') { console.log(usage); process.exit(0); }
  if (key === '--exit-after-start') { options.exitAfterStart = true; continue; }
  const field = { '--pane': 'pane', '--command': 'command', '--frame': 'frame', '--manifest': 'manifest' }[key];
  if (!field || index + 1 >= process.argv.length) throw new Error(usage);
  options[field] = process.argv[++index];
}
if (!['0', '1'].includes(options.pane)) throw new Error(usage);
options.pane = Number(options.pane);
options.frame = Number(options.frame);
if (!Number.isInteger(options.frame) || options.frame < 1 || options.frame > 60) throw new Error(usage);
const commands = new Set(['load', 'start', 'stop', 'edit', 'compose', 'delete', 'replace', 'stats', 'select']);
if (options.command && !commands.has(options.command)) throw new Error(usage);
if (!process.stdout.isTTY || !process.stdin.isTTY) throw new Error('Run inside an isolated native lyrel PTY (local or remote); redirected stdin/stdout cannot preload this workload');

const width = 1920, height = 1080, frameCount = 60;
const frameBytes = width * height * 4;
const seed = [0x1460a501, 0x1461b702][options.pane];
const imageId = 146000 + options.pane;
const source = await readFile(new URL(import.meta.url));
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const sourceSha256 = sha256(source);
const definition = { version: 1, sourceSha256, pane: options.pane, seed, imageId, width, height, frameCount, format: 'RGBA8', algorithm: 'xorshift32-rgb-gradient-v1', barcode: { x: 32, y: 32, width: 256, height: 32, bits: 8, msbFirst: true }, gapMs: [17, 17, 16] };
const fixtureIdentity = sha256(JSON.stringify(definition));
const records = [];
const counters = { attemptedUploads: 0, uploads: 0, rawBytes: 0, wireBytes: 0, stdoutBytes: 0, protocolCommands: 0, acknowledgedCommands: 0 };
const peakMemory = { rss: 0, heapUsed: 0, external: 0, arrayBuffers: 0 };
let pixels = null;
let generation = 0;
let loadedThisProcess = false;
let commandSerial = 0;
let manifestWritten = false;
let pendingAck = null;
let incoming = '';
let line = '';
let interrupted = false;
let queued = [];
let wakeCommand = null;
const originalRaw = process.stdin.isRaw;

function sampleMemory() {
  const memory = process.memoryUsage();
  for (const key of Object.keys(peakMemory)) peakMemory[key] = Math.max(peakMemory[key], memory[key]);
  return memory;
}
async function emit(text) {
  const length = Buffer.byteLength(text);
  counters.stdoutBytes += length;
  if (text.includes('\x1b_G')) counters.wireBytes += length;
  if (!process.stdout.write(text)) await once(process.stdout, 'drain');
  if (interrupted) throw new Error('Interrupted; accepted frames remain daemon-owned; delete explicitly if desired');
}
async function status(event, extra = {}) {
  const state = { event, phase: event, commandSerial, pid: process.pid, pane: options.pane, seed, imageId, sourceSha256, sourceBytes: source.length, fixtureIdentity, width, height, frameCount, ...counters, uploadedFrames: counters.uploads, bytes: counters.rawBytes, producerFrameBuffers: pixels ? 1 : 0, producerFrameBytes: pixels?.length ?? 0, maxBase64ChunkBytes: 4096, stdoutQueuedBytes: process.stdout.writableLength, memory: sampleMemory(), sampledPeakMemory: { ...peakMemory }, ...extra };
  // Keep diagnostic/ordinary input rows above the picture, without scrolling it.
  const marker = event === 'ready' ? 'KITTY_CAPACITY_READY' : 'KITTY_CAPACITY_STATE';
  await emit(`\x1b7\x1b[1;1H\x1b[2K\x1b[?7l${marker} ${JSON.stringify(state)}\r\n\x1b[2KCAP146:${commandSerial}:${event}:${counters.uploads}:${counters.wireBytes}\x1b[?7h\x1b8`);
  return state;
}
async function saveManifest(phase, error = null) {
  if (!options.manifest || manifestWritten) return;
  const manifest = { definition, fixtureIdentity, sourceBytes: source.length, producerPid: process.pid, phase, records, ...counters, sampledPeakMemory: peakMemory, loadedThisProcess, error };
  // Publish the complete source identity once, before the load completion marker.
  // Later edits cannot silently rewrite the baseline used by native calibration.
  await writeFile(options.manifest, `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx' });
  manifestWritten = true;
}
function receive(chunk) {
  incoming += chunk.toString('utf8');
  while (incoming.length) {
    if (incoming.startsWith('\x1b_G')) {
      const end = incoming.indexOf('\x1b\\');
      if (end < 0) break;
      const reply = incoming.slice(3, end);
      incoming = incoming.slice(end + 2);
      const separator = reply.indexOf(';');
      const keys = reply.slice(0, separator).split(',');
      const ack = pendingAck;
      if (!ack || !keys.includes(`i=${imageId}`)) { fail(new Error(`Unexpected Kitty reply: ${reply}`)); continue; }
      pendingAck = null;
      clearTimeout(ack.timer);
      if (reply.slice(separator + 1) === 'OK') { counters.acknowledgedCommands++; ack.resolve(); }
      else ack.reject(new Error(`Kitty rejected workload: ${reply}`));
      continue;
    }
    // Preserve an APC prefix split across native PTY reads.
    if ('\x1b_G'.startsWith(incoming)) break;
    const character = incoming[0];
    incoming = incoming.slice(1);
    if (character === '\x03') { interrupted = true; fail(new Error('Interrupted')); continue; }
    if (character === '\r' || character === '\n') {
      if (line.length) {
        if (queued.length >= 64) { fail(new Error('Too many pending input commands')); continue; }
        queued.push(line); line = ''; wakeCommand?.(); wakeCommand = null;
      }
    } else if (character === '\x7f' || character === '\b') line = line.slice(0, -1);
    else if (character >= ' ' && character !== '\x1b') {
      if (line.length >= 4096) { fail(new Error('Input command exceeds 4096 characters')); continue; }
      line += character;
    }
  }
  if (incoming.length > 16384) fail(new Error('Unterminated Kitty reply exceeds 16 KiB'));
}
let inputFailure = null;
function fail(error) {
  inputFailure ??= error;
  if (pendingAck) { clearTimeout(pendingAck.timer); pendingAck.reject(error); pendingAck = null; }
  wakeCommand?.(); wakeCommand = null;
}
async function acknowledged(send) {
  if (inputFailure) throw inputFailure;
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  // Observe early rejections while stdout is still draining a large frame.
  promise.catch(() => {});
  const ack = { resolve, reject, timer: setTimeout(() => fail(new Error('Kitty ACK timeout after 60 seconds')), 60000) };
  pendingAck = ack;
  try { counters.protocolCommands++; await send(); await promise; }
  finally { clearTimeout(ack.timer); if (pendingAck === ack) pendingAck = null; }
}
async function control(keys) {
  await acknowledged(() => emit(`\x1b_Ga=a,i=${imageId},q=0,${keys}\x1b\\`));
}
function fill(frame, version) {
  pixels ??= Buffer.allocUnsafe(frameBytes);
  let random = (seed ^ Math.imul(frame + 1, 0x9e3779b9) ^ Math.imul(version, 0x85ebca6b)) >>> 0;
  for (let y = 0, offset = 0; y < height; y++) {
    for (let x = 0; x < width; x++, offset += 4) {
      random ^= random << 13; random ^= random >>> 17; random ^= random << 5;
      pixels[offset] = (random + x + frame * 7) & 255;
      pixels[offset + 1] = ((random >>> 8) + y + version * 11) & 255;
      pixels[offset + 2] = ((random >>> 16) + x + y + options.pane * 61) & 255;
      pixels[offset + 3] = 255;
    }
  }
  // A white quiet border surrounds eight 32x32 MSB-first black/white bits.
  for (let y = 24; y < 72; y++) for (let x = 24; x < 296; x++) {
    const inside = x >= 32 && x < 288 && y >= 32 && y < 64;
    const white = !inside || Boolean(frame & (1 << (7 - ((x - 32) >> 5))));
    const offset = (y * width + x) * 4;
    pixels[offset] = pixels[offset + 1] = pixels[offset + 2] = white ? 255 : 0;
  }
  sampleMemory();
}
function gap(frame) { return [17, 17, 16][frame % 3]; }
async function upload(frame, action, version = 0, target = null) {
  fill(frame, version);
  const digest = sha256(pixels);
  counters.attemptedUploads++;
  await acknowledged(async () => {
    for (let offset = 0; offset < pixels.length; offset += 3072) {
      // 3072 raw bytes = 4096 Base64 bytes: never allocate a whole encoded frame.
      const chunk = pixels.subarray(offset, Math.min(offset + 3072, pixels.length)).toString('base64');
      const more = Number(offset + 3072 < pixels.length);
      const header = offset === 0
        ? `a=${action},i=${imageId},f=32,s=${width},v=${height},${target === null ? `z=${gap(frame)},` : `r=${target},X=1,`}q=0,m=${more}`
        : `${action === 'f' ? 'a=f,' : ''}m=${more}`;
      await emit(`\x1b_G${header};${chunk}\x1b\\`);
    }
  });
  counters.uploads++;
  counters.rawBytes += frameBytes;
  if (action === 't') records.length = 0;
  records[target === null ? frame : target - 1] = { operation: action === 't' ? 'root' : target === null ? 'append' : 'edit', frameId: frame, target, version, rawBytes: frameBytes, sha256: digest };
  sampleMemory();
}
async function place() {
  const columns = Math.max(1, (process.stdout.columns || 80) - 2);
  const rows = Math.max(1, (process.stdout.rows || 40) - 6);
  await acknowledged(() => emit(`\x1b[5;2H\x1b_Ga=p,i=${imageId},p=1,c=${columns},r=${rows},C=1,q=0\x1b\\`));
  await emit('\x1b[3;1H');
}
async function load() {
  generation = 0;
  for (let frame = 0; frame < frameCount; frame++) {
    await upload(frame, frame === 0 ? 't' : 'f');
    await status('loading', { acceptedFramesThisLoad: frame + 1 });
  }
  // Root transmission ignores z; set its interval explicitly before starting.
  await control(`r=1,z=${gap(0)},s=1,c=1`);
  await place();
  loadedThisProcess = true;
  await saveManifest('loaded');
  await status('loaded', { acceptedFramesThisLoad: frameCount, retainedRawWorkloadBytes: frameCount * frameBytes });
}
function frameNumber(text, fallback) {
  const value = text === undefined ? fallback : Number(text);
  if (!Number.isInteger(value) || value < 1 || value > frameCount) throw new Error('Frame must be 1..60 (Kitty is 1-based; barcode is 0-based)');
  return value;
}
async function execute(text) {
  commandSerial++;
  const [command, first, second, ...rest] = text.trim().split(/\s+/);
  if (command === 'load') await load();
  else if (command === 'start') await control('s=3,v=1');
  else if (command === 'stop') await control('s=1');
  else if (command === 'select') await control(`s=1,c=${frameNumber(first, options.frame)}`);
  else if (command === 'edit') {
    const target = frameNumber(first, options.frame);
    await upload(target - 1, 'f', ++generation, target);
  } else if (command === 'compose') {
    const from = frameNumber(first, 2), to = frameNumber(second, 3);
    if (from === to) throw new Error('Compose requires different source and target frames');
    await acknowledged(() => emit(`\x1b_Ga=c,i=${imageId},r=${from},c=${to},C=1,q=0\x1b\\`));
    records[to - 1] = { operation: 'compose', frameId: to - 1, sourceFrame: from, targetFrame: to, rawBytes: frameBytes, sha256: records[from - 1]?.sha256 ?? null };
  } else if (command === 'replace') {
    // Deliberately replace the complete root, retire the old 60-frame sequence,
    // then append the other 59 full-size versions and resume daemon playback.
    generation++;
    for (let frame = 0; frame < frameCount; frame++) await upload(frame, frame === 0 ? 't' : 'f', generation);
    await control(`r=1,z=${gap(0)},s=1,c=1`);
    await place(); await control('s=3,v=1'); loadedThisProcess = true;
  } else if (command === 'delete') {
    // Successful delete has no reply. A non-retaining query is an ordered barrier.
    await acknowledged(async () => {
      await emit(`\x1b_Ga=d,d=I,i=${imageId},q=0\x1b\\`);
      await emit(`\x1b_Ga=q,i=${imageId},f=32,s=1,v=1,q=0;AAAA/w==\x1b\\`);
    });
    loadedThisProcess = false;
  } else if (command === 'input') {
    const token = [first, second, ...rest].filter(value => value !== undefined).join(' ');
    await status('ordinary-input', { token, receivedAt: new Date().toISOString() });
  } else if (command !== 'stats') throw new Error('Commands: load/start/stop/select N/edit N/compose SOURCE TARGET/replace/delete/input TEXT/stats/quit');
  await status(command, { loadedThisProcess });
}

process.stdin.setRawMode(true);
process.stdin.on('data', receive);
process.stdin.on('error', fail);
process.stdin.on('end', () => fail(new Error('PTY input ended')));
process.stdin.resume();
let failure = null;
try {
  if (options.command) await execute(`${options.command}${['edit', 'select'].includes(options.command) ? ` ${options.frame}` : ''}`);
  else {
    if (options.exitAfterStart) { await execute('load'); await execute('start'); }
    else {
      await emit('\x1b[2;1H\x1b[2Kload/start/stop/edit N/compose A B/replace/delete/input TEXT/stats/quit\x1b[3;1H');
      await status('ready');
      for (;;) {
        if (inputFailure) throw inputFailure;
        if (!queued.length) await new Promise(resolve => { wakeCommand = resolve; });
        if (inputFailure) throw inputFailure;
        const command = queued.shift();
        if (command.trim() === 'quit') { commandSerial++; await status('quit'); break; }
        await execute(command);
      }
    }
  }
} catch (error) { failure = error; process.exitCode = 1; }
finally {
  pixels = null;
  // No implicit stop/delete on exit: the living daemon, not this process, plays.
  if (!interrupted) await status(failure ? 'failed' : 'sender-exited', { error: failure?.message ?? null, loadedThisProcess });
  process.stdin.off('data', receive);
  process.stdin.setRawMode(originalRaw ?? false);
  process.stdin.pause();
  await saveManifest(failure ? 'failed' : 'sender-exited', failure?.message ?? null);
  process.stdout.write('\x1b[?7h\x1b[3;1H');
}
if (failure) process.stderr.write(`KITTY_CAPACITY_ERROR ${failure.message}\n`);
