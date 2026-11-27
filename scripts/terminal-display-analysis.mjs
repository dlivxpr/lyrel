#!/usr/bin/env node
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { distribution, parseCsv } from './kitty-capacity-analysis.mjs';

function records(text, required) {
  const [header, ...rows] = parseCsv(text.replace(/^\uFEFF/, '').trimEnd());
  if (!header || required.some((column) => !header.includes(column))) {
    throw new Error(`CSV must include ${required.join(', ')}`);
  }
  return rows.map((row) => {
    if (row.length !== header.length) throw new Error('CSV row/header length mismatch');
    return Object.fromEntries(header.map((column, index) => [column, row[index]]));
  });
}

function milliseconds(value, column, unavailable = false) {
  if (unavailable && (value === 'NA' || value === '')) return null;
  const parsed = Number(value);
  if (!value.trim() || !Number.isFinite(parsed) || parsed < 0) {
    throw new Error(`Invalid ${column}: ${value}`);
  }
  return parsed;
}

function timedEvents(text) {
  let previous = -1;
  return records(text, ['id', 'qpc_ms']).map(({ id, qpc_ms }) => {
    const time = milliseconds(qpc_ms, 'qpc_ms');
    if (time < previous) throw new Error('Evidence is not in QPC order');
    previous = time;
    return { id, time };
  });
}

export function analyzeDisplay(inputText, captureText, presentText, { pid, refreshHz, swapchain }) {
  if (!Number.isInteger(pid) || pid < 1 || !Number.isFinite(refreshHz) || refreshHz <= 0) {
    throw new Error('Provide a positive GUI PID and fixed display refresh rate');
  }
  const inputs = timedEvents(inputText);
  const inputIds = new Set(inputs.map(({ id }) => id));
  if (!inputs.length || inputIds.has('') || inputIds.size !== inputs.length) {
    throw new Error('Expected at least one input and a unique nonempty ID for every input');
  }
  const captures = new Map();
  const observations = timedEvents(captureText);
  for (const { id, time } of observations) {
    if (!id) continue;
    if (!captures.has(id)) captures.set(id, []);
    captures.get(id).push(time);
  }
  const frameTimes = [...new Set(observations.map(({ time }) => time))];

  const byChain = new Map();
  const presentRecords = presentText === null ? [] : records(presentText,
    ['ProcessID', 'SwapChainAddress', 'CPUStartQPCTime', 'DisplayLatency', 'DisplayedTime']);
  for (const row of presentRecords) {
    if (Number(row.ProcessID) !== pid) continue;
    if (!byChain.has(row.SwapChainAddress)) {
      byChain.set(row.SwapChainAddress, { displayed: [], notDisplayed: 0 });
    }
    const chain = byChain.get(row.SwapChainAddress);
    const latency = milliseconds(row.DisplayLatency, 'DisplayLatency', true);
    const displayed = milliseconds(row.DisplayedTime, 'DisplayedTime', true);
    if (latency === null || displayed === null) {
      chain.notDisplayed++;
      continue;
    }
    chain.displayed.push(milliseconds(row.CPUStartQPCTime, 'CPUStartQPCTime') + latency);
  }
  const active = [...byChain].filter(([, chain]) => chain.displayed.length).map(([id]) => id);
  if (!swapchain && active.length > 1) {
    throw new Error(`Multiple displayed swapchains for PID ${pid}: ${active.join(', ')}; specify --swapchain`);
  }
  const selected = swapchain ?? active[0] ?? (byChain.size === 1 ? byChain.keys().next().value : null);
  if (swapchain && !byChain.has(swapchain)) throw new Error(`No ${swapchain} swapchain for PID ${pid}`);
  const chain = byChain.get(selected) ?? { displayed: [], notDisplayed: 0 };
  const displayTimes = chain.displayed.sort((a, b) => a - b);
  const refreshMs = 1000 / refreshHz;
  const matchingRadiusMs = refreshMs / 2;
  const observedDisplayTimes = displayTimes.filter((time) => time >= frameTimes[0] && time <= frameTimes.at(-1));
  const intervals = observedDisplayTimes.slice(1).map((time, i) => time - observedDisplayTimes[i]);
  const captureByPresent = new Map();
  const captured = [];
  const results = inputs.map(({ id, time: inputQpcMs }) => {
    const copies = captures.get(id) ?? [];
    const captureQpcMs = copies[0] ?? null;
    let reason;
    if (captureQpcMs === null) reason = 'marker_missing';
    else if (captureQpcMs < inputQpcMs) reason = 'stale_marker';
    else if (copies.length !== 1) reason = 'duplicate_marker';
    else {
      captured.push(captureQpcMs - inputQpcMs);
      const priorCapture = frameTimes.findLast((time) => time < captureQpcMs);
      // A present after the last observed old content could precede the WGC
      // sample. If capture omitted a frame, proximity alone does not identify it.
      const earliest = Math.max(inputQpcMs, (priorCapture ?? inputQpcMs) - matchingRadiusMs);
      const potential = displayTimes.filter((time) => time >= earliest && time <= captureQpcMs + matchingRadiusMs);
      if (potential.length === 0) reason = 'present_missing';
      else if (potential.length !== 1) reason = 'ambiguous_present';
      else if (Math.abs(potential[0] - captureQpcMs) > matchingRadiusMs) reason = 'present_outside_tolerance';
      else {
        const displayQpcMs = potential[0];
        const previous = captureByPresent.get(displayQpcMs);
        captureByPresent.set(displayQpcMs,
          previous === undefined || previous === captureQpcMs ? captureQpcMs : null);
        return { id, inputQpcMs, captureQpcMs, displayQpcMs, latencyMs: displayQpcMs - inputQpcMs };
      }
    }
    return { id, inputQpcMs, captureQpcMs, reason };
  });
  const matched = [];
  const unmatched = [];
  for (const result of results) {
    if ('reason' in result) unmatched.push(result);
    else if (captureByPresent.get(result.displayQpcMs) === null) {
      // One display event can corroborate several markers in the same
      // captured frame, but it cannot identify two distinct captured frames.
      const { id, inputQpcMs, captureQpcMs } = result;
      unmatched.push({ id, inputQpcMs, captureQpcMs, reason: 'shared_present' });
    } else matched.push(result);
  }
  return {
    pid, refreshHz, matchingRadiusMs, swapchain: selected,
    presentMonAvailable: presentText !== null,
    recordedInputs: inputs.length,
    captureFrames: frameTimes.length,
    presentedFrames: observedDisplayTimes.length,
    notDisplayed: chain.notDisplayed,
    displayIntervalsMs: distribution(intervals),
    framesOverTwoRefreshPeriods: intervals.filter((ms) => ms > 2 * refreshMs).length,
    displayLatencyMs: distribution(matched.map(({ latencyMs }) => latencyMs)),
    windowCaptureLatencyMs: distribution(captured),
    unexpectedMarkers: [...captures.keys()].filter((id) => !inputIds.has(id)),
    matched, unmatched,
  };
}

async function main() {
  const args = process.argv.slice(2);
  const options = new Map();
  for (let i = 0; i < args.length; i += 2) {
    const [name, value] = args.slice(i, i + 2);
    if (!['--inputs', '--frames', '--presentmon', '--pid', '--refresh-hz', '--swapchain'].includes(name)
      || !value || options.has(name)) throw new Error(`Invalid argument ${name}`);
    options.set(name, value);
  }
  for (const name of ['--inputs', '--frames', '--pid', '--refresh-hz']) {
    if (!options.has(name)) throw new Error(`Missing ${name}`);
  }
  const [inputs, frames, present] = await Promise.all([
    readFile(options.get('--inputs'), 'utf8'), readFile(options.get('--frames'), 'utf8'),
    options.has('--presentmon') ? readFile(options.get('--presentmon'), 'utf8') : null,
  ]);
  const result = analyzeDisplay(inputs, frames, present, {
    pid: Number(options.get('--pid')),
    refreshHz: Number(options.get('--refresh-hz')),
    swapchain: options.get('--swapchain'),
  });
  console.log(JSON.stringify(result, null, 2));
  if (result.unmatched.length || result.unexpectedMarkers.length) process.exitCode = 2;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
