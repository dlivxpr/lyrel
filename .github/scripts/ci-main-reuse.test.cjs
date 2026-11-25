'use strict';

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, rmSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const { discoverComparableRun, GATE_FILES, PLATFORM_JOBS, verifyRun } = require('./ci-main-reuse.cjs');

const CI_WORKFLOW = 'name: CI\n';
const RELEASE_WORKFLOW = 'name: Release build\n';
const WORKFLOW_PATH = '.github/workflows/ci.yml';
const DAY_MS = 24 * 60 * 60 * 1000;
const NOW = Date.parse('2026-01-01T00:00:00Z');
const TREE = 'd'.repeat(40);
const HEAD = 'a'.repeat(40);
const BASE = 'b'.repeat(40);

// 纯校验：不碰 git、不碰网络，快照出 verifyRun 的完整证据契约。
function evidence() {
  const repository = { id: 7, full_name: 'owner/repo', fork: false };
  const gate = Object.fromEntries(GATE_FILES.map((file, index) => [file, String(index + 1).padStart(40, '0')]));
  const job = { head_sha: HEAD, run_attempt: 2, status: 'completed', conclusion: 'success' };
  const request = { number: 1, state: 'closed', merged_at: new Date(NOW).toISOString(),
    head: { sha: HEAD, repo: { id: 7 } }, base: { ref: 'main', sha: BASE, repo: { id: 7 } } };
  return {
    repository: 'owner/repo',
    sourceTree: TREE,
    gate: { ...gate },
    workflow: { id: 9, path: WORKFLOW_PATH },
    request,
    run: {
      id: 4242,
      run_attempt: 2,
      workflow_id: 9,
      path: WORKFLOW_PATH,
      event: 'pull_request',
      status: 'completed',
      conclusion: 'success',
      head_sha: HEAD,
      created_at: new Date(NOW - 5 * 60 * 1000).toISOString(),
      repository,
      head_repository: { ...repository },
      pull_requests: [],
      referenced_workflows: [{ path: `owner/repo/.github/workflows/release-build.yml@${'c'.repeat(40)}`,
        ref: 'refs/pull/1/merge', sha: 'c'.repeat(40) }],
    },
    jobs: [
      { ...job, name: 'preflight' },
      ...PLATFORM_JOBS.map(name => ({ ...job, name })),
      { ...job, name: 'lyrel-server musl (x86_64-unknown-linux-musl)', conclusion: 'skipped' },
      { ...job, name: 'release-validation / build (x86_64-pc-windows-msvc)', conclusion: 'skipped' },
    ],
    merge: { commit: 'c'.repeat(40), base: BASE, head: HEAD, tree: TREE, baseGate: { ...gate } },
    now: NOW,
  };
}

test('a completed three-platform pull request gate on the same tree is accepted', () => {
  assert.deepEqual(verifyRun(evidence()), { runId: '4242', runAttempt: '2', pullRequest: 1 });
});

for (const [name, mutate] of [
  ['a fork cannot stand in for this repository', value => { value.run.head_repository = { ...value.run.head_repository, fork: true }; }],
  ['a different repository full name is rejected', value => { value.run.repository = { ...value.run.repository, full_name: 'other/repo' }; }],
  ['another workflow path is rejected', value => { value.run.path = '.github/workflows/release.yml'; }],
  ['a different workflow id is rejected', value => { value.run.workflow_id = 10; }],
  ['a non-PR event is rejected', value => { value.run.event = 'push'; }],
  ['an unfinished run is rejected', value => { value.run.conclusion = 'failure'; }],
  ['an abandoned attempt is rejected', value => { value.run.run_attempt = 3; }],
  ['an unmerged pull request is rejected', value => { value.request.merged_at = null; }],
  ['missing immutable workflow evidence is rejected', value => { value.run.referenced_workflows = []; }],
  ['ambiguous immutable workflow evidence is rejected', value => { value.run.referenced_workflows.push(value.run.referenced_workflows[0]); }],
  ['another PR merge reference is rejected', value => { value.run.referenced_workflows[0].ref = 'refs/pull/2/merge'; }],
  ['a foreign referenced repository is rejected', value => { value.run.referenced_workflows[0].path = value.run.referenced_workflows[0].path.replace('owner/repo', 'foreign/repo'); }],
  ['stale evidence is rejected', value => { value.run.created_at = new Date(NOW - 4 * DAY_MS).toISOString(); }],
  ['evidence from the future is rejected', value => { value.run.created_at = new Date(NOW + DAY_MS).toISOString(); }],
  ['a missing created_at is rejected', value => { delete value.run.created_at; }],
  ['a failed platform job is rejected', value => { value.jobs.find(job => job.name === PLATFORM_JOBS[0]).conclusion = 'failure'; }],
  ['a skipped platform job is rejected', value => { value.jobs.find(job => job.name === PLATFORM_JOBS[1]).conclusion = 'skipped'; }],
  ['a missing preflight is rejected', value => { value.jobs = value.jobs.filter(job => job.name !== 'preflight'); }],
  ['a duplicate platform job is rejected', value => { value.jobs.push({ ...value.jobs[1] }); }],
  ['a job from another attempt is rejected', value => { value.jobs[0].run_attempt = 1; }],
  ['a job for another SHA is rejected', value => { value.jobs[1].head_sha = 'e'.repeat(40); }],
  ['an unfinished job is rejected', value => { value.jobs.push({ ...value.jobs[0], name: 'server', status: 'in_progress' }); }],
  ['a different main tree is rejected', value => { value.sourceTree = 'f'.repeat(40); }],
  ['a merge for another base is rejected', value => { value.merge.base = 'e'.repeat(40); }],
  ['a merge for another head is rejected', value => { value.merge.head = 'e'.repeat(40); }],
  ['a changed gate definition is rejected', value => { value.merge.baseGate[WORKFLOW_PATH] = 'e'.repeat(40); }],
  ['a gate definition missing at the base is rejected', value => { delete value.merge.baseGate[GATE_FILES[1]]; }],
]) {
  test(name, () => {
    const value = evidence();
    mutate(value);
    assert.throws(() => verifyRun(value));
  });
}

// 真实 git fixture：merge ref 的抓取、tree 与门禁 blob 的比较都必须来自真对象。
function repository(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'lyrel-ci-main-reuse-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const remote = path.join(directory, 'remote.git');
  const work = path.join(directory, 'work');
  const run = (cwd, ...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  run(directory, 'init', '--quiet', '--bare', remote);
  run(directory, 'init', '--quiet', '--initial-branch=main', work);
  run(work, 'config', 'user.name', 'CI fixture');
  run(work, 'config', 'user.email', 'ci@example.invalid');
  run(work, 'config', 'commit.gpgsign', 'false');
  run(work, 'remote', 'add', 'origin', remote);
  const commit = (files, message = 'fixture') => {
    for (const [name, content] of Object.entries(files)) {
      const file = path.join(work, name);
      mkdirSync(path.dirname(file), { recursive: true });
      writeFileSync(file, content);
    }
    run(work, 'add', '.');
    run(work, 'commit', '--quiet', '-m', message);
    return run(work, 'rev-parse', 'HEAD');
  };
  commit({
    '.github/workflows/ci.yml': CI_WORKFLOW,
    '.github/workflows/release-build.yml': RELEASE_WORKFLOW,
    'src/lib.rs': 'pub fn base() {}\n',
  });

  function pullRequest({ files = { 'src/feature.rs': 'pub fn feature() {}\n' }, number = 1 } = {}) {
    run(work, 'checkout', '--quiet', '-b', 'feature');
    const head = commit(files, 'feature');
    run(work, 'checkout', '--quiet', 'main');
    const base = run(work, 'rev-parse', 'HEAD');
    run(work, 'merge', '--quiet', '--no-ff', 'feature', '-m', 'test merge');
    const merge = run(work, 'rev-parse', 'HEAD');
    run(work, 'push', '--quiet', 'origin', 'main');
    run(work, 'push', '--quiet', 'origin', `HEAD:refs/pull/${number}/merge`);
    return { base, head, merge, tree: run(work, 'rev-parse', 'HEAD^{tree}') };
  }

  return { git: (...args) => run(work, ...args), pullRequest, advance: files => commit(files, 'advance') };
}

function record(pull) {
  const value = evidence();
  value.run.head_sha = pull.head;
  value.run.created_at = new Date(Date.now() - 5 * 60 * 1000).toISOString();
  value.request = { ...value.request, head: { sha: pull.head, repo: { id: 7 } },
    base: { ref: 'main', sha: pull.base, repo: { id: 7 } } };
  value.run.pull_requests = [value.request];
  value.run.referenced_workflows = [{ path: `owner/repo/.github/workflows/release-build.yml@${pull.merge}`,
    ref: 'refs/pull/1/merge', sha: pull.merge }];
  value.jobs = value.jobs.map(job => ({ ...job, head_sha: pull.head }));
  return { run: value.run, jobs: value.jobs };
}

async function discover({ work, sourceSha, run, jobs, workflow = { id: 9, path: WORKFLOW_PATH } }) {
  const github = {
    rest: {
      actions: {
        getWorkflow: async () => ({ data: workflow }),
        listWorkflowRuns: async () => ({ data: { workflow_runs: [run] } }),
        listJobsForWorkflowRun: async () => { throw Error('jobs must come from paginate'); },
      },
      repos: { listPullRequestsAssociatedWithCommit: async () => ({ data: run.pull_requests }) },
      git: { getCommit: async ({ commit_sha }) => ({ data: {
        sha: commit_sha, tree: { sha: work.git('rev-parse', `${commit_sha}^{tree}`) },
        parents: work.git('show', '-s', '--format=%P', commit_sha).split(' ').map(sha => ({ sha })),
      } }) },
    },
    paginate: async (route) => route === github.rest.repos.listPullRequestsAssociatedWithCommit ? run.pull_requests : jobs,
  };
  const reasons = [];
  const result = await discoverComparableRun({
    github,
    repo: { owner: 'owner', repo: 'repo' },
    sourceSha: sourceSha ?? work.git('rev-parse', 'HEAD'),
    git: (...args) => work.git(...args),
    log: message => reasons.push(message),
  });
  return { result, reasons: reasons.join('\n') };
}

test('the real merge ref and tree of a matching pull request run are reused', async (t) => {
  const work = repository(t);
  const pull = work.pullRequest({});
  const { result } = await discover({ work, ...record(pull) });
  assert.deepEqual(result, { reuse: true, releaseReuse: false, runId: '4242', runAttempt: '2', pullRequest: 1, tree: pull.tree });
});

test('release-validation is reusable only when all release build and package jobs succeeded', async (t) => {
  const work = repository(t);
  const pull = work.pullRequest({});
  const value = record(pull);
  const prefix = 'release-validation / ';
  const release = [
    'build (x86_64-pc-windows-msvc)', 'build (aarch64-apple-darwin)',
    'pack (x86_64-pc-windows-msvc)', 'pack (aarch64-apple-darwin)',
    'server-musl (x86_64-unknown-linux-musl, lyrel-server-linux-x86_64-musl)',
    'server-musl (aarch64-unknown-linux-musl, lyrel-server-linux-aarch64-musl)',
    'server-macos (aarch64-apple-darwin, arm64, lyrel-server-macos-aarch64)',
    'server-windows (x86_64-pc-windows-msvc, lyrel-server-windows-x86_64.zip)',
  ].map(name => ({ ...value.jobs[0], name: `${prefix}${name}` }));
  value.jobs = value.jobs.filter(job => !job.name.startsWith(prefix));
  value.jobs.push(...release.slice(1));
  assert.equal((await discover({ work, ...value })).result.releaseReuse, false);
  value.jobs.push(release[0]);
  assert.equal((await discover({ work, ...value })).result.releaseReuse, true);
  value.jobs = value.jobs.filter(job => !PLATFORM_JOBS.includes(job.name));
  const scoped = (await discover({ work, ...value })).result;
  assert.equal(scoped.reuse, false);
  assert.equal(scoped.releaseReuse, true);
});

test('a different main tree is never accepted on the strength of the run alone', async (t) => {
  const work = repository(t);
  const pull = work.pullRequest({});
  work.advance({ 'src/after-merge.rs': 'pub fn later() {}\n' });
  const { result, reasons } = await discover({ work, ...record(pull) });
  assert.equal(result.reuse, false);
  assert.match(reasons, /tree differs/);
});

test('a workflow-only comment change preserves comparable test evidence', async (t) => {
  const work = repository(t);
  const pull = work.pullRequest({ files: { [WORKFLOW_PATH]: `# maintainer note\n${CI_WORKFLOW}` } });
  const { result } = await discover({ work, ...record(pull) });
  assert.equal(result.reuse, true);
});

test('gate definitions changed by the pull request cannot reuse main evidence', async (t) => {
  const work = repository(t);
  const pull = work.pullRequest({ files: { [WORKFLOW_PATH]: 'name: Narrowed CI\n' } });
  const { result, reasons } = await discover({ work, ...record(pull) });
  assert.equal(result.reuse, false);
  assert.match(reasons, /gate definition changed/);
});

test('an unreadable immutable merge commit falls back instead of trusting the run record', async (t) => {
  const work = repository(t);
  const pull = work.pullRequest({});
  const value = record(pull);
  value.run.referenced_workflows[0] = { path: `owner/repo/.github/workflows/release-build.yml@${'f'.repeat(40)}`,
    ref: 'refs/pull/1/merge', sha: 'f'.repeat(40) };
  const { result } = await discover({ work, ...value });
  assert.equal(result.reuse, false);
});

test('a broken checkout is an optimization miss, not a build failure', async () => {
  const result = await discoverComparableRun({
    github: { rest: { actions: { getWorkflow: async () => ({ data: {} }) } }, paginate: async () => [] },
    repo: { owner: 'owner', repo: 'repo' },
    sourceSha: HEAD,
    git: (...args) => { throw Error(`unexpected git call: ${args.join(' ')}`); },
    log: () => {},
  });
  assert.deepEqual(result, { reuse: false });
});

test('an API outage is an optimization miss, not a build failure', async (t) => {
  const work = repository(t);
  work.pullRequest({});
  const result = await discoverComparableRun({
    github: {
      rest: { actions: { getWorkflow: async () => { throw Error('service unavailable'); } } },
      paginate: async () => { throw Error('not reached'); },
    },
    repo: { owner: 'owner', repo: 'repo' },
    sourceSha: work.git('rev-parse', 'HEAD'),
    git: (...args) => work.git(...args),
    log: message => assert.match(message, /service unavailable/),
  });
  assert.deepEqual(result, { reuse: false });
});

test('a checkout that is not the event SHA is never used as evidence', async (t) => {
  const work = repository(t);
  const pull = work.pullRequest({});
  const { result } = await discover({ work, sourceSha: 'c'.repeat(40), ...record(pull) });
  assert.equal(result.reuse, false);
});

test('a PR associated with the squash commit can prove the tested tree after its merge ref is deleted', async (t) => {
  const work = repository(t);
  const pull = work.pullRequest({});
  const { run, jobs } = record(pull);
  const request = {
    ...run.pull_requests[0], state: 'closed', merged_at: new Date().toISOString(),
    base: { ...run.pull_requests[0].base, ref: 'main' },
    head: { sha: pull.head, repo: { id: 7 } },
  };
  run.pull_requests = [];
  run.referenced_workflows = [{
    path: `owner/repo/.github/workflows/release-build.yml@${pull.merge}`,
    ref: 'refs/pull/1/merge', sha: pull.merge,
  }];
  work.git('push', '--quiet', 'origin', ':refs/pull/1/merge');
  const squash = work.git('commit-tree', pull.tree, '-p', pull.base, '-m', 'squash');
  work.git('update-ref', 'refs/heads/main', squash);
  request.merge_commit_sha = squash;
  const github = {
    rest: {
      actions: {
        getWorkflow: async () => ({ data: { id: 9, path: WORKFLOW_PATH } }),
        listWorkflowRuns: async () => ({ data: { workflow_runs: [run] } }),
        listJobsForWorkflowRun: async () => ({ data: { jobs } }),
      },
      repos: { listPullRequestsAssociatedWithCommit: async () => ({ data: [request] }) },
      git: { getCommit: async ({ commit_sha }) => {
        assert.equal(commit_sha, pull.merge);
        return { data: { sha: pull.merge, tree: { sha: pull.tree }, parents: [{ sha: pull.base }, { sha: pull.head }] } };
      } },
    },
    paginate: async (route, params) => route === github.rest.repos.listPullRequestsAssociatedWithCommit
      ? (params.commit_sha === squash ? [request] : []) : jobs,
  };
  const result = await discoverComparableRun({
    github, repo: { owner: 'owner', repo: 'repo' }, sourceSha: squash,
    git: (...args) => work.git(...args), log: () => {},
  });
  assert.deepEqual(result, { reuse: true, releaseReuse: false, runId: '4242', runAttempt: '2', pullRequest: 1, tree: pull.tree });
});

test('a version-only push needs executed tests at its immediate main parent', async (t) => {
  const work = repository(t);
  const parent = work.git('rev-parse', 'HEAD');
  const source = work.advance({ 'Cargo.toml': '[workspace.package]\nversion = "1.0.1"\n' });
  const value = evidence();
  const run = { ...value.run, head_sha: parent, event: 'push', head_branch: 'main' };
  const jobs = value.jobs.slice(0, 4).map(job => ({ ...job, head_sha: parent, steps: [
    { name: 'Test desktop library and headless', status: 'completed', conclusion: 'success' },
    { name: 'Test Linux headless', status: 'completed', conclusion: 'success' },
  ] }));
  const github = {
    rest: {
      actions: {
        getWorkflow: async () => ({ data: value.workflow }),
        listWorkflowRuns: async ({ event, head_sha }) => ({ data: {
          workflow_runs: event === 'push' && head_sha === parent ? [run] : [],
        } }),
        listJobsForWorkflowRun: () => {},
      },
      repos: { listPullRequestsAssociatedWithCommit: () => {} },
    },
    paginate: async (route) => route === github.rest.actions.listJobsForWorkflowRun ? jobs : [],
  };
  const check = () => discoverComparableRun({
    github, repo: { owner: 'owner', repo: 'repo' }, sourceSha: source, versionParent: parent,
    git: work.git, now: NOW, log: () => {},
  });
  assert.equal((await check()).versionReuse, true);
  jobs[1].steps[0].conclusion = 'skipped';
  assert.notEqual((await check()).versionReuse, true);
});
