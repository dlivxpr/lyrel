// One-shot, SGR-only replay-ring flood for the real SSH restoration contract.
// It writes more than the production 8 MiB ring, then exits 0. It emits no
// OSC, DCS, image or colour command: after the original producer has exited,
// the accepted pane authority is the only source for a fresh consumer.
import fs from 'node:fs';

const line = Buffer.from('\x1b[0m'.repeat(250) + '\r\n');
const block = Buffer.concat(Array.from({ length: 100 }, () => line));
let bytes = 0;

function writeAll(buffer) {
  let offset = 0;
  while (offset < buffer.length) {
    offset += fs.writeSync(1, buffer, offset, buffer.length - offset);
  }
  bytes += buffer.length;
}

for (let i = 0; i < 92; i++) writeAll(block);
fs.writeSync(1, Buffer.from(`\r\nFLOOD_BYTES=${bytes}\r\nFLOOD_DONE\r\n`));
