'use strict';

const assert = require('node:assert/strict');
const { spawn, spawnSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, writeFileSync, readFileSync, copyFileSync, existsSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const root = path.resolve(__dirname, '../..');
const wrapper = path.join(root, 'scripts/cargo-windows.ps1');
const windows = process.platform === 'win32';

function fixture(t) {
  const dir = mkdtempSync(path.join(tmpdir(), 'lyrel-cargo-wrapper-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const bin = path.join(dir, 'bin');
  mkdirSync(bin);
  // This executable models Cargo's public JSON stream, not wrapper internals.
  writeFileSync(path.join(bin, 'cargo.cmd'), '@echo off\r\nnode "%~dp0cargo.cjs" %*\r\n');
  writeFileSync(path.join(bin, 'cargo.cjs'), `
    const fs = require('node:fs');
    fs.appendFileSync(process.env.CARGO_FIXTURE_CALLS, JSON.stringify(process.argv.slice(2)) + '\\n');
    console.log(JSON.stringify({ reason: 'compiler-message', message: { rendered: 'fixture diagnostic\\n' } }));
    console.log('fixture non-JSON output');
    if (process.env.CARGO_FIXTURE_RELEASE) {
      const deadline = Date.now() + 5000;
      const timer = setInterval(() => {
        if (fs.existsSync(process.env.CARGO_FIXTURE_RELEASE)) { clearInterval(timer); process.exit(0); }
        if (Date.now() > deadline) { clearInterval(timer); process.exit(71); }
      }, 20);
    } else {
      setTimeout(() => process.exit(Number(process.env.CARGO_FIXTURE_EXIT || 0)), 10);
    }
  `);
  return {
    dir,
    env: {
      ...process.env,
      PATH: `${bin}${path.delimiter}${process.env.PATH}`,
      CARGO_FIXTURE_CALLS: path.join(dir, 'calls.jsonl'),
    },
  };
}

test('Windows Cargo wrapper streams rendered diagnostics before Cargo exits', { skip: !windows }, async (t) => {
  const f = fixture(t);
  f.env.CARGO_FIXTURE_RELEASE = path.join(f.dir, 'diagnostic-received');
  const child = spawn('pwsh', ['-NoProfile', '-File', wrapper, 'test', '--locked', '--lib'], {
    cwd: root, env: f.env, stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  let released = false;
  for (const stream of [child.stdout, child.stderr]) stream.on('data', (bytes) => {
    output += bytes;
    if (output.includes('fixture diagnostic') && !released) {
      released = true;
      writeFileSync(f.env.CARGO_FIXTURE_RELEASE, 'continue');
    }
  });
  const code = await new Promise((resolve, reject) => {
    child.on('error', reject);
    child.on('close', resolve);
  });
  assert.equal(code, 0, output);
  assert.equal(released, true, output);
  assert.match(output, /fixture non-JSON output/);
  assert.doesNotMatch(output, /"reason":"compiler-message"/, 'diagnostics must be rendered, not leaked as JSON');
  const calls = readFileSync(f.env.CARGO_FIXTURE_CALLS, 'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(calls.length, 1, 'test execution must not invoke Cargo a second time');
});

test('Windows Cargo wrapper preserves a failed Cargo exit status', { skip: !windows }, (t) => {
  const f = fixture(t);
  const result = spawnSync('pwsh', ['-NoProfile', '-File', wrapper, 'build', '--locked'], {
    cwd: root, env: { ...f.env, CARGO_FIXTURE_EXIT: '37' }, encoding: 'utf8',
  });
  assert.equal(result.status, 37, result.stdout + result.stderr);
});

test('Windows Cargo wrapper rejects equal-length corruption before invoking Cargo', { skip: !windows }, (t) => {
  const f = fixture(t);
  const prepared = path.resolve(root, process.env.LYREL_CONPTY_DIR || '.conpty');
  const isolated = path.join(f.dir, 'prepared');
  mkdirSync(path.join(isolated, 'x64'), { recursive: true });
  for (const file of ['x64/conpty.dll', 'x64/OpenConsole.exe', 'LICENSE-ConPTY.txt']) {
    copyFileSync(path.join(prepared, file), path.join(isolated, file));
  }
  const dll = path.join(isolated, 'x64/conpty.dll');
  const bytes = readFileSync(dll);
  bytes[bytes.length - 1] ^= 1;
  writeFileSync(dll, bytes);
  const result = spawnSync('pwsh', ['-NoProfile', '-File', wrapper, 'build', '--locked'], {
    cwd: root, env: { ...f.env, LYREL_CONPTY_DIR: isolated }, encoding: 'utf8',
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stdout + result.stderr, /missing or mismatched ConPTY file/);
  assert.equal(existsSync(f.env.CARGO_FIXTURE_CALLS), false);
});
