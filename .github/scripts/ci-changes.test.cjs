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
  ['CLI changes do not rebuild the standalone server', { 'crates/lyrel-cli/src/main.rs': 'fn main() {}' }, [true, true, false, false]],
  ['pure documentation pushes retain only preflight', { 'docs/start.mdx': '# Guide', 'CHANGELOG.md': '# Changes' }, [false, false, false, false]],
  ['embedded license changes rebuild every Rust target', { LICENSE: 'license' }, [true, true, true, false]],
  ['lockfile changes rebuild every Rust target', { 'Cargo.lock': '# lock' }, [true, true, true, false]],
  ['CLI manifests can alter workspace resolution and require all Rust targets', { 'crates/lyrel-cli/Cargo.toml': '[package]\nname = "lyrel-cli"' }, [true, true, true, false]],
  ['unknown and embedded skill inputs are not treated as documentation', { 'skills/example/SKILL.md': '# Skill' }, [true, true, true, false]],
  ['shared binary assertions also validate the release pipeline', { '.github/scripts/assert-static.sh': 'exit 1' }, [true, true, true, true]],
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
  assert.deepEqual(scopes(repo.classify({}, after, 'workflow_dispatch')), [true, true, true, true]);
  assert.deepEqual(scopes(repo.classify({ before: repo.before, after }, repo.before)), [true, true, true, true]);
  assert.deepEqual(scopes(repo.classify({ pull_request: { base: { sha: repo.before }, head: { sha: after } } }, after, 'pull_request')),
    [true, true, true, true]);
});
