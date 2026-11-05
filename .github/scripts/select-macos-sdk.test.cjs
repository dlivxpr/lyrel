'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { mkdtempSync, readFileSync, rmSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const selector = path.resolve(__dirname, 'select-macos-sdk.sh').replaceAll('\\', '/');

// Exercise the shell entry point at its external-tool seam, never a real Xcode switch.
function select(t, overrides = {}) {
  const directory = mkdtempSync(path.join(tmpdir(), 'lyrel-sdk-')).replaceAll('\\', '/');
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const driver = `${directory}/driver.sh`;
  const log = `${directory}/tools.log`;
  const exported = `${directory}/github-env`;
  writeFileSync(log, '');
  writeFileSync(exported, '');
  writeFileSync(driver, `#!/bin/bash
xcode-select() {
  printf 'xcode-select %s\\n' "$*" >> "$TEST_LOG"
  [[ "$*" == '-p' ]] || return 91
  printf '%s\\n' /fixture/CurrentXcode/Contents/Developer
}
xcodebuild() {
  printf 'xcodebuild %s\\n' "$*" >> "$TEST_LOG"
  printf '%s\\n' "Xcode $TEST_SDK" 'Build version fixture'
}
xcrun() {
  printf 'xcrun %s developer=%s\\n' "$*" "$DEVELOPER_DIR" >> "$TEST_LOG"
  case "$*" in
    '--sdk macosx --show-sdk-version') printf '%s\\n' "$TEST_SDK" ;;
    '--sdk macosx --show-sdk-path') printf '%s\\n' "$DEVELOPER_DIR/SDKs/MacOSX.sdk" ;;
    '--sdk macosx --find clang')
      if [[ "$TEST_MISSING" == clang ]]; then return 2; fi
      printf '%s\\n' "$DEVELOPER_DIR/usr/bin/clang" ;;
    '--sdk macosx --find metal')
      if [[ "$TEST_MISSING" == metal ]]; then return 2; fi
      printf '%s\\n' "$DEVELOPER_DIR/usr/bin/metal" ;;
    *) return 92 ;;
  esac
}
source "$1"
`);
  const result = spawnSync('bash', [driver, selector], {
    encoding: 'utf8',
    env: { ...process.env, DEVELOPER_DIR: '/fixture/ExplicitXcode/Contents/Developer',
      TEST_SDK: '26.1', TEST_MISSING: '', TEST_LOG: log, GITHUB_ENV: exported, ...overrides },
  });
  assert.ifError(result.error);
  return { ...result, calls: readFileSync(log, 'utf8'), exported: readFileSync(exported, 'utf8') };
}

test('a compatible explicit Xcode exports its actual SDK and the desktop runtime floor', (t) => {
  const result = select(t);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /macOS SDK=26\.1/);
  assert.match(result.exported, /DEVELOPER_DIR=\/fixture\/ExplicitXcode\/Contents\/Developer\n/);
  assert.match(result.exported, /SDKROOT=\/fixture\/ExplicitXcode\/Contents\/Developer\/SDKs\/MacOSX\.sdk\n/);
  assert.match(result.exported, /MACOSX_DEPLOYMENT_TARGET=15\.0\n/);
  assert.doesNotMatch(result.calls, /xcode-select/);
});

test('an explicitly selected old SDK fails without silently choosing a different Xcode', (t) => {
  const result = select(t, { TEST_SDK: '25.4' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /DEVELOPER_DIR=.*does not provide macOS SDK 26 or newer/);
  assert.equal(result.exported, '');
  assert.doesNotMatch(result.calls, /xcode-select/);
});

test('automatic selection checks the actual current SDK without changing global Xcode selection', (t) => {
  const result = select(t, { DEVELOPER_DIR: '' });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.exported, /DEVELOPER_DIR=\/fixture\/CurrentXcode\/Contents\/Developer/);
  assert.match(result.calls, /xcode-select -p/);
  assert.doesNotMatch(result.calls, /xcode-select --?s/);
});

test('without any compatible SDK the prerequisite fails and exports no toolchain', (t) => {
  const result = select(t, { DEVELOPER_DIR: '', TEST_SDK: '25.4' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /No compatible Xcode is installed/);
  assert.equal(result.exported, '');
});

test('a missing required clang fails before publishing the environment', (t) => {
  const result = select(t, { TEST_MISSING: 'clang' });
  assert.notEqual(result.status, 0);
  assert.equal(result.exported, '');
});

test('runtime shaders do not require the offline metal compiler', (t) => {
  const result = select(t, { TEST_MISSING: 'metal' });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /metal compiler unavailable/);
  assert.match(result.exported, /MACOSX_DEPLOYMENT_TARGET=15\.0/);
});
