'use strict';

const assert = require('node:assert/strict');
const { execFileSync, spawnSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const script = path.resolve(__dirname, 'ci-changes.cjs');

function repository(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'lyrel-ci-changes-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: directory, encoding: 'utf8' }).trim();
  git('init', '--quiet', '--initial-branch=main');
  git('config', 'user.name', 'CI fixture');
  git('config', 'user.email', 'ci@example.invalid');
  const commit = (files) => {
    for (const [name, content] of Object.entries(files)) {
      const file = path.join(directory, name);
      mkdirSync(path.dirname(file), { recursive: true });
      writeFileSync(file, content);
    }
    git('add', '.');
    git('commit', '--quiet', '-m', 'fixture');
    return git('rev-parse', 'HEAD');
  };
  const before = commit({ 'src/main.rs': 'fn main() {}', 'docs/start.mdx': '# Start' });
  function classify(event, sha = git('rev-parse', 'HEAD'), eventName = 'push') {
    const eventFile = path.join(directory, '.git', 'event.json');
    writeFileSync(eventFile, JSON.stringify(event));
    const result = spawnSync(process.execPath, [script], {
      cwd: directory,
      encoding: 'utf8',
      env: { ...process.env, GITHUB_EVENT_NAME: eventName, GITHUB_EVENT_PATH: eventFile,
        GITHUB_SHA: sha, GITHUB_OUTPUT: '' },
    });
    assert.equal(result.status, 0, result.stderr);
    return JSON.parse(result.stdout);
  }
  return { git, commit, before, classify };
}

function scopes(result) {
  return [result.desktop, result.linux, result.server, result.release];
}

// PR 事件的分类对象是合并结果，因此 fixture 也必须真的构造一个双父 merge。
function pullRequest(t, { files, draft, labels = [], action = 'synchronize', label } = {}) {
  const repo = repository(t);
  repo.git('checkout', '--quiet', '-b', 'feature');
  const head = repo.commit(files);
  repo.git('checkout', '--quiet', 'main');
  const base = repo.git('rev-parse', 'HEAD');
  repo.git('merge', '--quiet', '--no-ff', 'feature', '-m', 'test merge');
  const merge = repo.git('rev-parse', 'HEAD');
  const request = { base: { sha: base }, head: { sha: head }, labels };
  if (draft !== undefined) request.draft = draft;
  const event = { action, pull_request: request };
  if (label !== undefined) event.label = { name: label };
  return { repo, event, merge };
}

test('a multi-commit push ending in documentation still validates the earlier core change', (t) => {
  const repo = repository(t);
  repo.commit({ 'crates/lyrel-core/src/lib.rs': 'pub fn changed() {}' });
  const after = repo.commit({ 'docs/start.mdx': '# Updated guide' });
  assert.deepEqual(scopes(repo.classify({ before: repo.before, after })), [true, true, true, false]);
});

test('release build workflow changes select real packaging validation, not unrelated Rust CI', (t) => {
  const repo = repository(t);
  const after = repo.commit({ '.github/workflows/release-build.yml': 'name: Production' });
  assert.deepEqual(scopes(repo.classify({ before: repo.before, after })), [false, false, false, true]);
});

for (const [name, files, expected] of [
  ['GUI changes keep both desktop platforms', { 'src/ui/app.rs': 'fn ui() {}' }, [true, false, false, false]],
  ['core integration tests still run on all test platforms', { 'crates/lyrel-core/tests/session.rs': 'fn test() {}' }, [true, true, false, false]],
  ['core daemon changes rebuild the headless deliverable and test consumers', { 'crates/lyrel-core/src/daemon/mod.rs': 'pub struct Daemon;' }, [true, true, true, false]],
  ['protocol source changes rebuild the standalone server and all test consumers', { 'crates/lyrel-protocol/src/daemon/protocol.rs': 'pub const PROTOCOL_VERSION: u32 = 19;' }, [true, true, true, false]],
  ['server source changes rebuild the headless deliverable and test consumers', { 'crates/lyrel-server/src/main.rs': 'fn main() {}' }, [true, true, true, false]],
  ['CLI changes do not rebuild the standalone server', { 'crates/lyrel-cli/src/main.rs': 'fn main() {}' }, [true, true, false, false]],
  ['pure documentation pushes retain only preflight', { 'docs/start.mdx': '# Guide', 'CHANGELOG.md': '# Changes' }, [false, false, false, false]],
  ['embedded license changes rebuild every Rust target', { LICENSE: 'license' }, [true, true, true, false]],
  ['lockfile changes rebuild every Rust target', { 'Cargo.lock': '# lock' }, [true, true, true, false]],
  ['CLI manifests can alter workspace resolution and require all Rust targets', { 'crates/lyrel-cli/Cargo.toml': '[package]\nname = "lyrel-cli"' }, [true, true, true, false]],
  ['unknown and embedded skill inputs are not treated as documentation', { 'skills/example/SKILL.md': '# Skill' }, [true, true, true, false]],
  ['shared binary assertions also validate the release pipeline', { '.github/scripts/assert-static.sh': 'exit 1' }, [true, true, true, true]],
  ['desktop SDK selection also validates release packaging', { '.github/scripts/select-macos-sdk.sh': 'exit 1' }, [true, true, true, true]],
  ['agent notes are not build inputs', { '.agent/pr-123.md': '# notes', '.agent/config.json': '{}' }, [false, false, false, false]],
  ['documentation site content is not a build input', { 'docs/docs.json': '{}', 'docs/theme.css': 'body {}' }, [false, false, false, false]],
  ['crate notices are not build inputs', { 'crates/lyrel-core/NOTICE.md': 'notice' }, [false, false, false, false]],
  ['the GUI-free document crate is validated on Linux too', { 'crates/lyrel-document/src/lib.rs': 'pub fn model() {}' }, [true, true, false, false]],
  ['the standalone updater crate stays on the desktop platforms', { 'crates/lyrel-updater/src/windows.rs': 'pub fn run() {}' }, [true, false, false, false]],
  ['shared desktop foundation source stays on the desktop platforms', { 'crates/lyrel-ui-base/src/host_ops.rs': 'pub fn run() {}' }, [true, false, false, false]],
  ['future desktop UI sub-crates stay on the desktop platforms', { 'crates/lyrel-terminal/src/lib.rs': 'pub fn model() {}' }, [true, false, false, false]],
  ['editor workbench source stays on the desktop platforms', { 'crates/lyrel-workbench-editor/src/lib.rs': 'pub fn model() {}' }, [true, false, false, false]],
  ['desktop foundation manifests can alter workspace resolution and require all Rust targets', { 'crates/lyrel-ui-base/Cargo.toml': '[package]\nname = "lyrel-ui-base"' }, [true, true, true, false]],
  ['shared test-support fixtures rebuild the headless consumers too', { 'crates/lyrel-test-support/src/pty.rs': 'pub fn pty_sender() {}' }, [true, true, false, false]],
  ['the GUI-free insights crate is validated on desktop and Linux headless', { 'crates/lyrel-insights/src/statistics/mod.rs': 'pub fn scan() {}' }, [true, true, false, false]],
  ['cargo configuration changes rebuild every Rust target', { '.cargo/config.toml': '[target.x86_64-unknown-linux-gnu]\n' }, [true, true, true, false]],
]) {
  test(name, (t) => {
    const repo = repository(t);
    const after = repo.commit(files);
    const result = repo.classify({ before: repo.before, after });
    assert.deepEqual(scopes(result), expected);
    assert.deepEqual(result.matrix.include.map(row => row.target), expected[0]
      ? ['aarch64-apple-darwin', 'x86_64-pc-windows-msvc', ...(expected[1] ? ['x86_64-unknown-linux-gnu'] : [])]
      : []);
  });
}

test('moving source into documentation still validates its deletion', (t) => {
  const repo = repository(t);
  repo.git('mv', 'src/main.rs', 'docs/former-source.md');
  repo.git('commit', '--quiet', '-m', 'move source');
  const after = repo.git('rev-parse', 'HEAD');
  assert.deepEqual(scopes(repo.classify({ before: repo.before, after })), [true, false, false, false]);
});

test('PR classification includes the whole tested merge instead of only its last source commit', (t) => {
  const repo = repository(t);
  repo.git('checkout', '--quiet', '-b', 'feature');
  repo.commit({ 'crates/lyrel-core/src/lib.rs': 'pub fn changed() {}' });
  const head = repo.commit({ 'docs/start.mdx': '# Final docs' });
  repo.git('checkout', '--quiet', 'main');
  const base = repo.commit({ 'README.md': '# Main changes' });
  repo.git('merge', '--quiet', '--no-ff', 'feature', '-m', 'test merge');
  const merge = repo.git('rev-parse', 'HEAD');
  assert.deepEqual(scopes(repo.classify({ pull_request: { base: { sha: base }, head: { sha: head } } }, merge, 'pull_request')),
    [true, true, true, false]);
});

test('unavailable, rewritten, mismatched, or manually selected ranges fall back to full validation', (t) => {
  const repo = repository(t);
  const after = repo.commit({ 'docs/start.mdx': '# Docs' });
  for (const event of [
    { before: '0'.repeat(40), after },
    { before: 'f'.repeat(40), after },
    { before: repo.before, after, forced: true },
    { before: repo.before, after: repo.before },
  ]) assert.deepEqual(scopes(repo.classify(event)), [true, true, true, true]);
  // 手动 dispatch 默认 scope=ci：完整矩阵照跑，但不再顺带打包发行产物。
  assert.deepEqual(scopes(repo.classify({}, after, 'workflow_dispatch')), [true, true, true, false]);
  assert.deepEqual(scopes(repo.classify({ inputs: { scope: 'full' } }, after, 'workflow_dispatch')), [true, true, true, true]);
  assert.deepEqual(scopes(repo.classify({ before: repo.before, after }, repo.before)), [true, true, true, true]);
  assert.deepEqual(scopes(repo.classify({ pull_request: { base: { sha: repo.before }, head: { sha: after } } }, after, 'pull_request')),
    [true, true, true, true]);
});

test('draft pull requests only run the fast Windows and Linux tier', (t) => {
  const { repo, event, merge } = pullRequest(t, { files: { 'Cargo.toml': '[workspace]\n' }, draft: true });
  const result = repo.classify(event, merge, 'pull_request');
  // Cargo 变更仍然覆盖桌面与 Linux，但快速层不跑 macOS、server 与发行打包。
  assert.deepEqual(scopes(result), [true, true, false, false]);
  assert.equal(result.tier, 'fast');
  assert.equal(result.production, false);
  assert.deepEqual(result.matrix.include.map(row => row.target),
    ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']);
});

test('ready pull requests run the full three-platform gate', (t) => {
  const { repo, event, merge } = pullRequest(t, { files: { 'Cargo.toml': '[workspace]\n' }, draft: false });
  const result = repo.classify(event, merge, 'pull_request');
  assert.deepEqual(scopes(result), [true, true, true, false]);
  assert.equal(result.tier, 'full');
  assert.equal(result.production, false);
  assert.deepEqual(result.matrix.include.map(row => row.target),
    ['aarch64-apple-darwin', 'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']);
});

test('a missing draft flag is treated as ready instead of silently downgrading', (t) => {
  const { repo, event, merge } = pullRequest(t, { files: { 'src/app.rs': 'fn app() {}' } });
  const result = repo.classify(event, merge, 'pull_request');
  assert.equal(result.tier, 'full');
  assert.deepEqual(result.matrix.include.map(row => row.target),
    ['aarch64-apple-darwin', 'x86_64-pc-windows-msvc']);
});

test('the ci:full label escalates a draft without waiting for review', (t) => {
  const { repo, event, merge } = pullRequest(t, {
    files: { 'src/app.rs': 'fn app() {}' },
    draft: true,
    labels: [{ name: 'ci:full' }],
    action: 'labeled',
    label: 'ci:full',
  });
  const result = repo.classify(event, merge, 'pull_request');
  assert.equal(result.tier, 'full');
  assert.deepEqual(scopes(result), [true, false, false, false]);
  assert.deepEqual(result.matrix.include.map(row => row.target),
    ['aarch64-apple-darwin', 'x86_64-pc-windows-msvc']);
});

test('an unrelated label never starts heavy validation', (t) => {
  const { repo, event, merge } = pullRequest(t, {
    files: { 'Cargo.toml': '[workspace]\n' },
    draft: false,
    action: 'labeled',
    label: 'documentation',
  });
  const result = repo.classify(event, merge, 'pull_request');
  assert.deepEqual(scopes(result), [false, false, false, false]);
  assert.deepEqual(result.matrix.include, []);
});

for (const [name, event, production] of [
  ['default ci', {}, false],
  ['explicit ci', { inputs: { scope: 'ci' } }, false],
  ['full', { inputs: { scope: 'full' } }, true],
]) {
  test(`manual ${name} keeps all platforms and selects the intended production build`, (t) => {
    const repo = repository(t);
    const result = repo.classify(event, undefined, 'workflow_dispatch');
    assert.equal(result.production, production);
    assert.equal(result.release, production);
    assert.equal(result.tier, 'full');
    assert.deepEqual(result.matrix.include.map(row => row.target),
      ['aarch64-apple-darwin', 'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']);
  });
}

test('main pushes retain the production build independently of the test tier', (t) => {
  const repo = repository(t);
  const after = repo.commit({ 'Cargo.toml': '[workspace]\n' });
  const result = repo.classify({ before: repo.before, after });
  assert.equal(result.production, true);
  assert.equal(result.tier, 'full');
});
