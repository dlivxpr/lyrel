#!/usr/bin/env node
// Analyze owned-window native evidence; never interprets paint callbacks as presents.
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

export function distribution(values) {
  const sorted = values.filter(Number.isFinite).sort((a, b) => a - b);
  if (!sorted.length) return { samples: 0, p50: null, p95: null, max: null };
  const percentile = (p) => sorted[Math.max(0, Math.ceil(sorted.length * p) - 1)];
  return { samples: sorted.length, p50: percentile(0.5), p95: percentile(0.95), max: sorted.at(-1) };
}

export function parseCsv(text) {
  const rows = [];
  let row = [], field = '', quoted = false;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (char === '"') {
      if (quoted && text[i + 1] === '"') { field += '"'; i++; }
      else if (quoted || field.length === 0) quoted = !quoted;
      else throw new Error('Unexpected quote inside CSV field');
    } else if (!quoted && (char === ',' || char === '\n')) {
      row.push(field.replace(/\r$/, '')); field = '';
      if (char === '\n') { if (row.some((item) => item !== '')) rows.push(row); row = []; }
    } else field += char;
  }
  if (quoted) throw new Error('Unterminated CSV quote');
  if (field.length || row.length) { row.push(field.replace(/\r$/, '')); rows.push(row); }
  return rows;
}

function numeric(value, field) {
  if (value === undefined || value === '' || value === 'NA') return null;
  const result = Number(value);
  if (!Number.isFinite(result)) throw new Error(`Invalid ${field}: ${value}`);
  return result;
}

export function analyzePresentMon(text) {
  const [header, ...rows] = parseCsv(text.replace(/^\uFEFF/, ''));
  if (!header?.includes('DisplayedTime')) throw new Error('PresentMon v2 CSV must include DisplayedTime; do not use --exclude_dropped');
  const records = rows.map((row) => {
    if (row.length !== header.length) throw new Error('PresentMon CSV row/header length mismatch');
    return Object.fromEntries(header.map((name, index) => [name, row[index]]));
  });
  if (!records.length) throw new Error('PresentMon CSV contains no events');
  const groups = new Map();
  for (const record of records) {
    const key = JSON.stringify([record.Application ?? '', record.ProcessID ?? '', record.SwapChainAddress ?? '']);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(record);
  }
  const summarize = (group) => {
    const metric = (name) => {
      const values = group.map((row) => numeric(row[name], name));
      const valid = values.filter((value) => value !== null);
      return { ...distribution(valid), unavailable: values.length - valid.length, unavailableRatio: (values.length - valid.length) / values.length };
    };
    const unseen = group.filter((row) => numeric(row.DisplayedTime, 'DisplayedTime') === null).length;
    return {
      application: group[0].Application ?? null,
      processId: group[0].ProcessID ?? null,
      swapchain: group[0].SwapChainAddress ?? null,
      presents: group.length,
      notDisplayed: unseen,
      notDisplayedRatio: unseen / group.length,
      runtimes: [...new Set(group.map((row) => row.PresentRuntime).filter(Boolean))],
      presentModes: [...new Set(group.map((row) => row.PresentMode).filter(Boolean))],
      displayedTimeMs: metric('DisplayedTime'),
      frameTimeMs: metric('FrameTime'),
      displayChangeMs: metric('MsBetweenDisplayChange'),
      inputToPhotonMs: metric('AllInputToPhotonLatency'),
      clickToPhotonMs: metric('ClickToPhotonLatency'),
    };
  };
  return { events: records.length, swapchains: [...groups.values()].map(summarize) };
}

export function barcodeMd5(frame) {
  const pixels = Buffer.from(Array.from({ length: 8 }, (_, bit) => (frame >> (7 - bit)) & 1 ? 255 : 0));
  return createHash('md5').update(pixels).digest('hex');
}

export function analyzeCapture(text, refreshHz = 60) {
  if (!Number.isFinite(refreshHz) || refreshHz <= 0) throw new Error('refresh-hz must be positive');
  const identities = new Map(Array.from({ length: 60 }, (_, id) => [barcodeMd5(id), id]));
  let timebase = null;
  const samples = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    if (line.startsWith('#')) {
      const match = /^#tb\s+0:\s*(\d+)\/(\d+)\s*$/.exec(line);
      if (match) {
        const numerator = Number(match[1]), denominator = Number(match[2]);
        if (!numerator || !denominator) throw new Error('Invalid capture timebase');
        timebase = numerator / denominator;
      }
      continue;
    }
    if (timebase === null) throw new Error('Capture must contain stream 0 timebase before frame rows');
    const fields = line.split(',').map((field) => field.trim());
    if (fields.length !== 6 || fields[0] !== '0' || fields[4] !== '8') throw new Error(`Expected stream 0, 8-byte 8x1 gray barcode: ${line}`);
    const pts = numeric(fields[2], 'capture PTS');
    const id = identities.get(fields[5].toLowerCase());
    if (id === undefined || pts === null) throw new Error(`Unknown/corrupt barcode or PTS: ${line}`);
    const ms = pts * timebase * 1000;
    if (samples.length && ms <= samples.at(-1).ms) throw new Error('Capture timestamps must be strictly increasing');
    samples.push({ ms, id });
  }
  if (samples.length < 2) throw new Error('Capture needs at least two barcode samples');
  const runs = [];
  for (const sample of samples) {
    if (runs.at(-1)?.id !== sample.id) runs.push({ ...sample });
  }
  const intervals = [], holds = [];
  let missingLowerBound = 0;
  for (let index = 1; index < runs.length; index++) {
    const previous = runs[index - 1], current = runs[index];
    const interval = current.ms - previous.ms;
    const delta = (current.id - previous.id + 60) % 60;
    const expectedMs = previous.id % 3 === 2 ? 16 : 17;
    intervals.push(interval);
    missingLowerBound += Math.max(0, delta - 1);
    if (interval > expectedMs + 1000 / refreshHz + 1) holds.push({ frame: previous.id, intervalMs: interval, expectedMs });
  }
  const observedMs = runs.length > 1 ? runs.at(-1).ms - runs[0].ms : 0;
  return {
    samples: samples.length,
    pictureChanges: Math.max(0, runs.length - 1),
    distinctFrameIds: [...new Set(samples.map((sample) => sample.id))].sort((a, b) => a - b),
    duplicateCaptureSamples: samples.length - runs.length,
    // A second pane/text update can repeat this pane's barcode in WGC. Collapse
    // those samples instead of falsely calling each compositor update a dropped frame.
    changeIntervalsMs: distribution(intervals),
    pictureChangesPerSecond: observedMs > 0 ? (runs.length - 1) * 1000 / observedMs : null,
    missingSourceFramesLowerBound: missingLowerBound,
    prolongedHolds: holds,
    firstPtsMs: samples[0].ms,
    lastPtsMs: samples.at(-1).ms,
    limits: 'WGC observes compositor output, not scanout. Jumps can include capture loss; whole missing loops cannot be inferred from modulo-60 IDs. Correlate with native PresentMon and input/resource observations.',
  };
}

async function main() {
  const options = new Map();
  for (let index = 2; index < process.argv.length; index += 2) {
    const name = process.argv[index], value = process.argv[index + 1];
    if (!['--presentmon', '--left', '--right', '--refresh-hz'].includes(name) || !value || options.has(name)) throw new Error('Usage: node scripts/kitty-capacity-analysis.mjs --presentmon FILE --left FILE --right FILE --refresh-hz HZ');
    options.set(name, value);
  }
  for (const required of ['--presentmon', '--left', '--right', '--refresh-hz']) if (!options.has(required)) throw new Error(`Missing ${required}`);
  const refreshHz = Number(options.get('--refresh-hz'));
  const [presentmon, left, right] = await Promise.all(['--presentmon', '--left', '--right'].map((name) => readFile(options.get(name), 'utf8')));
  console.log(JSON.stringify({
    presented: analyzePresentMon(presentmon),
    left: analyzeCapture(left, refreshHz),
    right: analyzeCapture(right, refreshHz),
    evidenceBoundary: 'No automatic performance pass. Record actual machine/build, visible pane geometry/scaling, display mode, source manifest, warmup/capture interval, input samples and separate producer/daemon/desktop/driver memory. Both native display timing and picture identity changes are required.',
  }, null, 2));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
