// #433 foreground PTY load. The only successful stop is a real SIGINT.
import { readFileSync, writeFileSync } from 'node:fs';

const [sample, tracePath] = process.argv.slice(2);
if (!/^\d{3}$/.test(sample) || !tracePath || !process.stdin.isTTY || !process.stdout.isTTY) {
  throw new Error('expected a numbered sample on a real Linux PTY and a trace path');
}

// See proc_pid_stat(5). The last ')' ends comm, which may itself contain ')'.
const stat = readFileSync('/proc/self/stat', 'utf8');
const fields = stat.slice(stat.lastIndexOf(') ') + 2).split(' ');
const pgrp = Number(fields[2]);
const foreground = Number(fields[5]);
if (!(pgrp > 0 && pgrp === foreground)) {
  throw new Error('expected the active terminal foreground process group');
}

const payload = 'x'.repeat(37);
const linesPerBurst = 128;
const intervalMs = 16;
let lines = 0;
const started = process.hrtime.bigint();
process.once('SIGINT', () => {
  const receipt = process.hrtime.bigint();
  clearInterval(output);
  clearTimeout(timeout);
  process.stdout.write(`LYREL433_ACK ${sample} ${lines}\n`, () => {
    writeFileSync(tracePath, `${JSON.stringify({
      sample, signal: 'SIGINT', pid: process.pid, pgrp, foreground, lines,
      startedNs: String(started), receiptNs: String(receipt),
      resourceUsage: process.resourceUsage(),
    })}\n`, { flag: 'wx', mode: 0o600 });
    process.exitCode = 130;
  });
});

process.stdout.write(`\nLYREL433_BEGIN ${sample} ${process.pid}\n`);
const output = setInterval(() => {
  let burst = '';
  for (let n = 0; n < linesPerBurst; ++n) {
    const row = String(++lines).padStart(6, '0');
    burst += `LYREL433_DATA ${sample} ${row} ${payload}\n`;
  }
  // Linux TTY writes are synchronous. An output-stalled PTY throttles the
  // producer and cannot accumulate an unbounded JS stream queue.
  process.stdout.write(burst);
}, intervalMs);
const timeout = setTimeout(() => {
  clearInterval(output);
  process.stderr.write(`LYREL433_TIMEOUT ${sample}\n`);
  process.exitCode = 124;
}, 8000);
