'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { selectServerArtifacts, discoverServerArtifacts, TARGET_JOBS } = require('./ci-server-reuse.cjs');
const SHA = 'a'.repeat(40);
function fixture() {
    const repository = { id: 7, full_name: 'owner/repo', fork: false };
    return {
        repository: 'owner/repo', sourceSha: SHA, now: Date.parse('2026-01-01'),
        workflow: { id: 9, path: '.github/workflows/ci.yml' },
        run: { id: 10, run_attempt: 2, repository, head_repository: repository, workflow_id: 9,
            path: '.github/workflows/ci.yml', head_sha: SHA, head_branch: 'main', event: 'push',
            status: 'completed', conclusion: 'success', pull_requests: [] },
        jobs: Object.values(TARGET_JOBS).map(name => ({ name, head_sha: SHA, status: 'completed', conclusion: 'success', run_attempt: 2 })),
        artifacts: Object.keys(TARGET_JOBS).map((target, index) => ({ id: 100 + index, name: `ci-server-${target}`,
            expired: false, size_in_bytes: 100, expires_at: '2026-02-01T00:00:00Z',
            workflow_run: { id: 10, repository_id: 7, head_repository_id: 7, head_sha: SHA } }))
    };
}
test('complete successful main CI yields immutable IDs for all three targets', () => {
    const selected = selectServerArtifacts(fixture());
    assert.equal(selected.runId, '10');
    assert.equal(selected.runAttempt, '2');
    assert.deepEqual(selected.artifactIds, {
        'x86_64-unknown-linux-musl': '100', 'aarch64-unknown-linux-musl': '101', 'aarch64-apple-darwin': '102'
    });
});
for (const [name, mutate] of [
    ['PR', f => { f.run.event = 'pull_request'; }],
    ['dispatch', f => { f.run.event = 'workflow_dispatch'; }],
    ['fork', f => { f.run.head_repository = { ...f.run.head_repository, fork: true }; }],
    ['different repository', f => { f.run.repository = { ...f.run.repository, full_name: 'attacker/repo' }; }],
    ['wrong workflow', f => { f.run.path = '.github/workflows/release.yml'; }],
    ['wrong SHA', f => { f.run.head_sha = 'b'.repeat(40); }],
    ['branch', f => { f.run.head_branch = 'feature'; }],
    ['failed CI', f => { f.run.conclusion = 'failure'; }],
    ['in-progress CI', f => { f.run.status = 'in_progress'; }],
    ['missing job', f => { f.jobs.pop(); }],
    ['skipped server', f => { f.jobs[0].conclusion = 'skipped'; }],
    ['old job attempt', f => { f.jobs[0].run_attempt = 1; }],
    ['missing artifact', f => { f.artifacts.pop(); }],
    ['duplicate artifact', f => { f.artifacts.push(f.artifacts[0]); }],
    ['expired flag', f => { f.artifacts[0].expired = true; }],
    ['expired date', f => { f.artifacts[0].expires_at = '2025-01-01'; }],
    ['unbound artifact', f => { delete f.artifacts[0].workflow_run; }],
    ['artifact SHA spoof', f => { f.artifacts[0].workflow_run.head_sha = 'b'.repeat(40); }],
    ['artifact repo spoof', f => { f.artifacts[0].workflow_run.repository_id = 77; }],
    ['artifact run spoof', f => { f.artifacts[0].workflow_run.id = 11; }]
]) test(`${name} falls back instead of accepting untrusted bytes`, () => {
    const value = fixture(); mutate(value);
    assert.throws(() => selectServerArtifacts(value));
});
test('API outage is an optimization miss, not a build failure', async () => {
    const reasons = [];
    const result = await discoverServerArtifacts({ github: { rest: { actions: { getWorkflow: async () => { throw Error('service unavailable'); } } } },
        repo: { owner: 'owner', repo: 'repo' }, sourceSha: SHA, log: message => reasons.push(message) });
    assert.equal(result.runId, '');
    assert.deepEqual(result.artifactIds, {});
    assert.match(reasons.join('\n'), /service unavailable/);
});
