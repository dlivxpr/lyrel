'use strict';

const assert = require('node:assert/strict');
const { execFile, execFileSync, spawnSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require('node:fs');
const { createServer } = require('node:http');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');
const { promisify } = require('node:util');

const script = path.resolve(__dirname, 'ci-changes.cjs');
const run = promisify(execFile);
const project = 'owner/repo';
const dispatchBranch = 'feature/dispatch';

function repository(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'lyrel-ci-changes-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: directory, encoding: 'utf8' }).trim();
  git('init', '--quiet', '--initial-branch=main');
  git('config', 'user.name', 'CI fixture');
  git('config', 'user.email', 'ci@example.invalid');
  git('config', 'commit.gpgsign', 'false');
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
  function invocation(event, sha, eventName, overrides) {
    const eventFile = path.join(directory, '.git', 'event.json');
    writeFileSync(eventFile, JSON.stringify(event));
    return {
      cwd: directory,
      encoding: 'utf8',
      env: { ...process.env, GITHUB_EVENT_NAME: eventName, GITHUB_EVENT_PATH: eventFile,
        GITHUB_SHA: sha, GITHUB_OUTPUT: '', GITHUB_REF: 'refs/heads/main', ...overrides },
    };
  }
  function classify(event, sha = git('rev-parse', 'HEAD'), eventName = 'push', overrides) {
    const result = spawnSync(process.execPath, [script], invocation(event, sha, eventName, overrides));
    assert.equal(result.status, 0, result.stderr);
    return JSON.parse(result.stdout);
  }
  async function dispatch(event, sha, apiUrl, overrides) {
    const output = path.join(directory, '.git', 'outputs');
    const { stdout } = await run(process.execPath, [script], invocation(event, sha, 'workflow_dispatch', {
      GITHUB_REF: `refs/heads/${dispatchBranch}`, GITHUB_REPOSITORY: project,
      GITHUB_API_URL: apiUrl, GITHUB_TOKEN: 'fixture-token', GITHUB_OUTPUT: output, ...overrides,
    }));
    const result = JSON.parse(stdout);
    const outputs = readFileSync(output, 'utf8');
    assert.ok(outputs.includes(`defer=${result.defer}\n`));
    assert.ok(outputs.includes(`heavy_key=${result.heavy_key}\n`));
    rmSync(output);
    return result;
  }
  return { git, commit, before, classify, dispatch };
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

function dispatchRepository(t) {
  const repo = repository(t);
  repo.git('update-ref', 'refs/remotes/origin/main', repo.before);
  repo.git('checkout', '--quiet', '-b', dispatchBranch);
  const event = { ref: `refs/heads/${dispatchBranch}`, repository: { full_name: project }, inputs: { scope: 'ci' } };
  return { repo, event };
}

function openPullRequest(sha) {
  return { state: 'open', head: { ref: dispatchBranch, sha, repo: { full_name: project } },
    base: { ref: 'main', repo: { full_name: project } } };
}

async function pullRequestApi(t, body, status = 200) {
  const requests = [];
  const server = createServer((request, response) => {
    requests.push({ method: request.method, url: new URL(request.url, 'http://localhost') });
    response.writeHead(status, { 'content-type': 'application/json' });
    response.end(JSON.stringify(body));
  });
  await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
  t.after(() => server.close());
  return { url: `http://127.0.0.1:${server.address().port}`, requests };
}

test('a manual run on an open same-repository pull request head defers all heavy jobs', async (t) => {
  const { repo, event } = dispatchRepository(t);
  const head = repo.commit({ 'Cargo.toml': '[workspace]\n' });
  const api = await pullRequestApi(t, [openPullRequest(head)]);
  const result = await repo.dispatch(event, head, api.url);
  assert.equal(result.defer, true);
  assert.deepEqual(scopes(result), [false, false, false, false]);
  assert.equal(result.production, false);
  assert.deepEqual(result.matrix.include, []);
  assert.equal(api.requests.length, 1);
  const [{ method, url }] = api.requests;
  assert.equal(method, 'GET');
  assert.equal(url.pathname, '/repos/owner/repo/pulls');
  assert.equal(url.searchParams.get('state'), 'open');
  assert.equal(url.searchParams.get('head'), 'owner:feature/dispatch');

  const full = await repo.dispatch({ ...event, inputs: { scope: 'full' } }, head, api.url);
  assert.equal(full.defer, true);
  assert.deepEqual(scopes(full), [false, false, false, false]);
  assert.equal(full.production, false);

  repo.git('checkout', '--quiet', 'main');
  repo.git('merge', '--quiet', '--no-ff', dispatchBranch, '-m', 'test merge');
  const request = openPullRequest(head);
  request.base.sha = repo.before;
  const pr = repo.classify({ action: 'synchronize', pull_request: request }, undefined, 'pull_request', { GITHUB_REPOSITORY: project });
  assert.match(result.heavy_key, new RegExp(`^${head}-[0-9a-f]{64}$`));
  assert.equal(pr.heavy_key, result.heavy_key);
  request.head.repo.full_name = 'owner/fork';
  const fork = repo.classify({ action: 'synchronize', pull_request: request }, undefined, 'pull_request', { GITHUB_REPOSITORY: project });
  assert.equal(fork.heavy_key, '');
});

test('a standalone manual run classifies against the main merge-base, excluding main-only changes', async (t) => {
  const { repo, event } = dispatchRepository(t);
  const head = repo.commit({ 'crates/lyrel-cli/src/main.rs': 'fn main() {}' });
  repo.git('checkout', '--quiet', 'main');
  const main = repo.commit({ '.cargo/config.toml': '[build]\n' });
  repo.git('update-ref', 'refs/remotes/origin/main', main);
  repo.git('checkout', '--quiet', dispatchBranch);
  const api = await pullRequestApi(t, []);
  const result = await repo.dispatch(event, head, api.url);
  assert.equal(result.defer, false);
  assert.deepEqual(scopes(result), [true, true, false, false]);
  assert.equal(result.tier, 'fast');
  assert.equal(result.production, false);
  assert.equal(result.fail_fast, false);
  assert.deepEqual(result.matrix.include.map(row => row.target),
    ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']);
});

test('only an open PR with the same repository, branch and head can replace manual validation', async (t) => {
  const { repo, event } = dispatchRepository(t);
  const head = repo.commit({ 'docs/start.mdx': '# Manual update' });
  const request = openPullRequest(head);
  const api = await pullRequestApi(t, [
    openPullRequest(repo.before),
    { ...request, head: { ...request.head, repo: { full_name: 'owner/fork' } } },
    { ...request, head: { ...request.head, ref: 'feature/other' } },
    { ...request, base: { ...request.base, repo: { full_name: 'other/repo' } } },
    { ...request, state: 'closed' },
  ]);
  const result = await repo.dispatch(event, head, api.url);
  assert.equal(result.defer, false);
  assert.deepEqual(scopes(result), [false, false, false, false]);
  assert.equal(result.tier, 'fast');
});

test('standalone manual scope full keeps every platform, the production build and packaging', async (t) => {
  const { repo, event } = dispatchRepository(t);
  const head = repo.commit({ 'docs/start.mdx': '# Manual full' });
  const api = await pullRequestApi(t, []);
  const result = await repo.dispatch({ ...event, inputs: { scope: 'full' } }, head, api.url);
  assert.equal(result.defer, false);
  assert.equal(result.production, true);
  assert.equal(result.fail_fast, false);
  assert.equal(result.tier, 'full');
  assert.deepEqual(scopes(result), [true, true, true, true]);
  assert.deepEqual(result.matrix.include.map(row => row.target),
    ['aarch64-apple-darwin', 'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']);
});

test('unavailable dispatch evidence validates fully without competing with a PR', async (t) => {
  const { repo, event } = dispatchRepository(t);
  const head = repo.commit({ 'docs/start.mdx': '# Manual update' });
  const unavailable = await pullRequestApi(t, { message: 'Forbidden' }, 403);
  const malformed = await pullRequestApi(t, [{ state: 'open', head: { sha: head } }]);
  const empty = await pullRequestApi(t, []);
  repo.git('update-ref', '-d', 'refs/remotes/origin/main');
  for (const api of [unavailable, malformed, empty]) {
    const result = await repo.dispatch(event, head, api.url);
    assert.equal(result.defer, false);
    assert.equal(result.heavy_key, '');
    assert.equal(result.tier, 'full');
    assert.deepEqual(scopes(result), [true, true, true, false]);
  }
  const result = await repo.dispatch({ ...event, inputs: { scope: 'unknown' } }, head, empty.url);
  assert.equal(result.defer, false);
  assert.equal(result.heavy_key, '');
  assert.deepEqual(scopes(result), [true, true, true, true]);
});

test('a multi-commit push ending in documentation still validates the earlier core change', (t) => {
  const repo = repository(t);
  repo.commit({ 'crates/lyrel-core/src/lib.rs': 'pub fn changed() {}' });
  const after = repo.commit({ 'docs/start.mdx': '# Updated guide' });
  assert.deepEqual(scopes(repo.classify({ before: repo.before, after })), [true, true, true, false]);
});

test('release build workflow changes select real packaging validation, not unrelated Rust CI', (t) => {
  const { repo, event, merge } = pullRequest(t, {
    files: { '.github/workflows/release-build.yml': 'name: Production' }, draft: false,
  });
  assert.deepEqual(scopes(repo.classify(event, merge, 'pull_request')), [false, false, false, true]);
});

for (const [name, files, expected] of [
  ['GUI changes keep both desktop platforms', { 'src/ui/app.rs': 'fn ui() {}' }, [true, false, false, false]],
  ['core integration tests still run on all test platforms', { 'crates/lyrel-core/tests/session.rs': 'fn test() {}' }, [true, true, false, false]],
  ['core daemon changes validate all test consumers without rebuilding the standalone server', { 'crates/lyrel-core/src/daemon/mod.rs': 'pub struct Daemon;' }, [true, true, false, false]],
  ['protocol changes validate all test consumers without rebuilding the standalone server', { 'crates/lyrel-protocol/src/daemon/protocol.rs': 'pub const PROTOCOL_VERSION: u32 = 19;' }, [true, true, false, false]],
  ['server source changes rebuild the headless deliverable and test consumers', { 'crates/lyrel-server/src/main.rs': 'fn main() {}' }, [true, true, true, false]],
  ['CLI changes do not rebuild the standalone server', { 'crates/lyrel-cli/src/main.rs': 'fn main() {}' }, [true, true, false, false]],
  ['pure documentation PRs retain only preflight', { 'docs/start.mdx': '# Guide', 'CHANGELOG.md': '# Changes' }, [false, false, false, false]],
  ['embedded license changes validate every test platform', { LICENSE: 'license' }, [true, true, false, false]],
  ['lockfile changes rebuild every Rust target', { 'Cargo.lock': '# lock' }, [true, true, true, false]],
  ['CLI manifests can alter workspace resolution and require all Rust targets', { 'crates/lyrel-cli/Cargo.toml': '[package]\nname = "lyrel-cli"' }, [true, true, true, false]],
  ['unknown and embedded skill inputs are not treated as documentation', { 'skills/example/SKILL.md': '# Skill' }, [true, true, false, false]],
  ['shared binary assertions also validate the release pipeline', { '.github/scripts/assert-static.sh': 'exit 1' }, [true, true, true, true]],
  ['Mach-O assertions validate the standalone server and release packaging', { '.github/scripts/assert-macho.sh': 'exit 1' }, [true, true, true, true]],
  ['desktop SDK selection also validates release packaging', { '.github/scripts/select-macos-sdk.sh': 'exit 1' }, [true, true, false, true]],
  ['agent notes are not build inputs', { '.agent/pr-123.md': '# notes', '.agent/config.json': '{}' }, [false, false, false, false]],
  ['documentation site content is not a build input', { 'docs/docs.json': '{}', 'docs/theme.css': 'body {}' }, [false, false, false, false]],
  ['crate notices are not build inputs', { 'crates/lyrel-core/NOTICE.md': 'notice' }, [false, false, false, false]],
  ['the GUI-free document crate is validated on Linux too', { 'crates/lyrel-document/src/lib.rs': 'pub fn model() {}' }, [true, true, false, false]],
  ['the standalone updater crate stays on the desktop platforms', { 'crates/lyrel-updater/src/windows.rs': 'pub fn run() {}' }, [true, false, false, false]],
  ['shared desktop foundation source stays on the desktop platforms', { 'crates/lyrel-ui-base/src/host_ops.rs': 'pub fn run() {}' }, [true, false, false, false]],
  ['future desktop UI sub-crates stay on the desktop platforms', { 'crates/lyrel-terminal/src/lib.rs': 'pub fn model() {}' }, [true, false, false, false]],
  ['editor workbench source stays on the desktop platforms', { 'crates/lyrel-workbench-editor/src/lib.rs': 'pub fn model() {}' }, [true, false, false, false]],
  ['native markdown editor source stays on the desktop platforms', { 'crates/lyrel-markdown-editor/src/lib.rs': 'pub fn model() {}' }, [true, false, false, false]],
  ['desktop foundation manifests can alter workspace resolution and require all Rust targets', { 'crates/lyrel-ui-base/Cargo.toml': '[package]\nname = "lyrel-ui-base"' }, [true, true, true, false]],
  ['shared test-support fixtures rebuild the headless consumers too', { 'crates/lyrel-test-support/src/pty.rs': 'pub fn pty_sender() {}' }, [true, true, false, false]],
  ['the GUI-free insights crate is validated on desktop and Linux headless', { 'crates/lyrel-insights/src/statistics/mod.rs': 'pub fn scan() {}' }, [true, true, false, false]],
  ['cargo configuration changes rebuild every Rust target', { '.cargo/config.toml': '[target.x86_64-unknown-linux-gnu]\n' }, [true, true, true, false]],
]) {
  test(name, (t) => {
    const { repo, event, merge } = pullRequest(t, { files, draft: false });
    const result = repo.classify(event, merge, 'pull_request');
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
  assert.deepEqual(scopes(repo.classify({ before: repo.before, after })), [true, false, true, false]);
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
    [true, true, false, false]);
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
  const pr = repo.classify({ action: 'synchronize', pull_request: {
    draft: true, labels: [], base: { sha: repo.before }, head: { sha: after },
  } }, after, 'pull_request');
  assert.deepEqual(scopes(pr), [true, true, true, true]);
  assert.equal(pr.fail_fast, false);
});

test('draft pull requests only run the fast Windows and Linux tier', (t) => {
  const { repo, event, merge } = pullRequest(t, { files: { 'Cargo.toml': '[workspace]\n' }, draft: true });
  const result = repo.classify(event, merge, 'pull_request');
  // Cargo 变更仍然覆盖桌面与 Linux，但快速层不跑 macOS、server 与发行打包。
  assert.deepEqual(scopes(result), [true, true, false, false]);
  assert.equal(result.tier, 'fast');
  assert.equal(result.fail_fast, true);
  assert.equal(result.production, false);
  assert.deepEqual(result.matrix.include.map(row => row.target),
    ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']);
});

test('ready pull requests run the full three-platform gate', (t) => {
  const { repo, event, merge } = pullRequest(t, { files: { 'Cargo.toml': '[workspace]\n' }, draft: false });
  const result = repo.classify(event, merge, 'pull_request');
  assert.deepEqual(scopes(result), [true, true, true, false]);
  assert.equal(result.tier, 'full');
  assert.equal(result.fail_fast, false);
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
  assert.equal(result.fail_fast, false);
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

test('main pushes keep the standalone server evidence even for preflight-only changes', (t) => {
  const repo = repository(t);
  const after = repo.commit({ 'docs/start.mdx': '# New guide' });
  const result = repo.classify({ before: repo.before, after });
  assert.deepEqual(scopes(result), [false, false, true, false]);
  assert.equal(result.fail_fast, false);
  assert.equal(result.production, true);
  assert.equal(result.tier, 'full');
});
