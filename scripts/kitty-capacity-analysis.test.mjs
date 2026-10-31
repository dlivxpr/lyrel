import { test } from 'node:test';
import assert from 'node:assert/strict';
import { analyzeCapture, analyzePresentMon, barcodeMd5, parseCsv } from './kitty-capacity-analysis.mjs';

function capture(samples) {
  return '#format: frame checksums\n#tb 0: 1/10000000\n' + samples.map(([ms, id]) => `0, ${ms * 10000}, ${ms * 10000}, 1, 8, ${barcodeMd5(id)}`).join('\n');
}

test('native source identities collapse other-pane compositor updates without calling them repeated pictures', () => {
  const result = analyzeCapture(capture([[0, 0], [4, 0], [17, 1], [34, 2], [50, 3]]), 240);
  assert.equal(result.duplicateCaptureSamples, 1);
  assert.equal(result.pictureChanges, 3);
  assert.equal(result.missingSourceFramesLowerBound, 0);
  assert.equal(result.changeIntervalsMs.samples, 3);
  for (const key of ['p50', 'p95', 'max']) {
    assert.ok(Math.abs(result.changeIntervalsMs[key] - 17) < 1e-9);
  }
  assert.deepEqual(result.prolongedHolds, []);
  assert.ok(Math.abs(result.pictureChangesPerSecond - 60) < 1e-9);
});

test('capture reports skipped source IDs and prolonged picture holds including wraparound', () => {
  const result = analyzeCapture(capture([[0, 58], [17, 59], [50, 1], [100, 2]]), 240);
  assert.equal(result.missingSourceFramesLowerBound, 1);
  assert.deepEqual(result.prolongedHolds.map(({ intervalMs, ...hold }) => hold), [
    { frame: 59, expectedMs: 16 },
    { frame: 1, expectedMs: 17 },
  ]);
  assert.ok(Math.abs(result.prolongedHolds[0].intervalMs - 33) < 1e-9);
  assert.ok(Math.abs(result.prolongedHolds[1].intervalMs - 50) < 1e-9);
});

test('invalid barcode, packet size, missing timebase and reordered timestamps fail closed', () => {
  assert.throws(() => analyzeCapture(capture([[0, 0], [17, 1]]).replace(barcodeMd5(1), '0'.repeat(32))), /Unknown\/corrupt/);
  assert.throws(() => analyzeCapture(capture([[0, 0], [17, 1]]).replace(', 8,', ', 4,')), /8-byte/);
  assert.throws(() => analyzeCapture(capture([[0, 0], [17, 1]]).replace('#tb 0: 1/10000000\n', '')), /timebase/);
  assert.throws(() => analyzeCapture(capture([[17, 0], [0, 1]])), /strictly increasing/);
});

test('PresentMon keeps not-displayed events and unavailable input separate across swapchains', () => {
  const result = analyzePresentMon([
    'Application,ProcessID,SwapChainAddress,DisplayedTime,FrameTime,AllInputToPhotonLatency,ClickToPhotonLatency,PresentRuntime',
    'lyrel-app.exe,42,0x1,16.7,16.7,8,NA,DXGI',
    'lyrel-app.exe,42,0x1,NA,33.4,NA,NA,DXGI',
    'lyrel-app.exe,42,0x2,8.3,8.3,NA,4,DXGI',
  ].join('\n'));
  assert.equal(result.events, 3);
  assert.equal(result.swapchains.length, 2);
  const first = result.swapchains[0];
  assert.equal(first.notDisplayed, 1);
  assert.equal(first.notDisplayedRatio, 0.5);
  assert.deepEqual(first.displayedTimeMs, { samples: 1, p50: 16.7, p95: 16.7, max: 16.7, unavailable: 1, unavailableRatio: 0.5 });
  assert.equal(first.inputToPhotonMs.p95, 8);
  assert.equal(first.inputToPhotonMs.unavailable, 1);
  assert.equal(first.clickToPhotonMs.samples, 0);
});

test('CSV respects quoted application names rather than silently shifting native fields', () => {
  assert.deepEqual(parseCsv('Application,Value\r\n"name, ""quoted""",1\r\n'), [['Application', 'Value'], ['name, "quoted"', '1']]);
  assert.throws(() => parseCsv('"unterminated'), /Unterminated/);
  assert.throws(() => analyzePresentMon('Application,FrameTime\na,16'), /DisplayedTime/);
});
