import assert from 'node:assert/strict';
import { test } from 'node:test';
import { analyzeDisplay } from './terminal-display-analysis.mjs';

const presentMon = (...rows) => [
  'ProcessID,SwapChainAddress,CPUStartQPCTime,DisplayLatency,DisplayedTime',
  ...rows,
].join('\n');
const evidence = (...rows) => ['id,qpc_ms', ...rows].join('\n');
const options = { pid: 42, refreshHz: 100 };

test('uses the first correct capture and a uniquely corroborating displayed frame', () => {
  const result = analyzeDisplay(
    evidence('>aaab>a>,1000', '>aaac>b>,1100'),
    evidence(',1002', '>aaab>a>,1011', ',1030', ',1100', '>aaac>b>,1121'),
    presentMon('42,0x1,990,20,20', '42,0x1,1000,NA,NA',
      '42,0x1,1010,20,10', '42,0x1,1100,20,5', '11,0x1,1110,13,9'),
    options,
  );
  assert.deepEqual(result.displayLatencyMs, { samples: 2, p50: 10, p95: 20, max: 20 });
  assert.equal(result.framesOverTwoRefreshPeriods, 1);
  assert.equal(result.notDisplayed, 1);
  assert.equal(result.unmatched.length, 0);
});

test('counts stale, duplicate, missing and ambiguous evidence as unmatched', () => {
  const result = analyzeDisplay(
    evidence('one,1000', 'two,1100', 'three,1200', 'four,1300', 'five,1400'),
    evidence('one,990', 'one,1010', 'two,1110', 'two,1120', 'wrong,1200', 'four,1310', 'five,1410'),
    presentMon('42,A,980,10,10', '42,A,1300,8,4', '42,A,1302,10,10', '42,A,1400,NA,NA'),
    options,
  );
  assert.deepEqual(result.unmatched.map((sample) => sample.reason), [
    'stale_marker', 'duplicate_marker', 'marker_missing', 'ambiguous_present', 'present_missing',
  ]);
  assert.deepEqual(result.unexpectedMarkers, ['wrong']);
  assert.equal(result.displayLatencyMs.p95, null);
});

test('requires an explicit target when multiple swapchains displayed and rejects mixed clocks', () => {
  const inputs = evidence('one,1000');
  const frames = evidence('one,1010');
  const presents = presentMon('42,A,1000,10,10', '42,B,1000,10,10');
  assert.throws(() => analyzeDisplay(inputs, frames, presents, options), /swapchain/i);
  assert.equal(analyzeDisplay(inputs, frames, presents, { ...options, swapchain: 'A' }).matched.length, 1);
  assert.throws(() => analyzeDisplay(inputs, frames,
    presents.replace('CPUStartQPCTime', 'CPUStartTime'), options), /CPUStartQPCTime/);
});

test('retains compositor-only observations without inventing displayed samples', () => {
  const result = analyzeDisplay(evidence('one,1000'), evidence(',990', 'one,1010'), null, options);
  assert.equal(result.windowCaptureLatencyMs.p95, 10);
  assert.equal(result.displayLatencyMs.p95, null);
  assert.equal(result.unmatched[0].reason, 'present_missing');
});

test('does not infer the first correct display when capture might have skipped it', () => {
  const result = analyzeDisplay(evidence('one,1000'), evidence(',1000', 'one,1030'),
    presentMon('42,A,1000,10,20', '42,A,1012,18,10'), options);
  assert.equal(result.unmatched[0].reason, 'ambiguous_present');
});

test('rejects a present claimed by different captured frames', () => {
  const result = analyzeDisplay(evidence('one,1000', 'two,1004'),
    evidence(',1000', 'one,1009', 'two,1012'), presentMon('42,A,1000,10,10'), options);
  assert.equal(result.windowCaptureLatencyMs.samples, 2);
  assert.equal(result.displayLatencyMs.samples, 0);
  assert.deepEqual(result.unmatched.map(({ id, reason }) => [id, reason]), [
    ['one', 'shared_present'], ['two', 'shared_present'],
  ]);
});

test('matches distinct input markers first seen in the same captured frame', () => {
  const result = analyzeDisplay(evidence('one,1000', 'two,1004'),
    evidence(',1000', 'one,1012', 'two,1012'), presentMon('42,A,1000,10,10'), options);
  assert.deepEqual(result.matched.map(({ id, latencyMs }) => [id, latencyMs]), [['one', 10], ['two', 6]]);
});
