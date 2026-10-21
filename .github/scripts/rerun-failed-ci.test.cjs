'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const { main } = require('../../scripts/rerun-failed-ci.cjs');

const RUN_ID = '39417499778';
const SHA = '0f605c4b9e46dc5febd8e5f9e4c76200abcfeb4c';

function fixture({ run = {}, jobs, repository = 'dlivxpr/lyrel', env = {} } = {}) {
    const state = {
        run: {
            id: Number(RUN_ID), repository: { full_name: repository },
            name: 'CI', path: '.github/workflows/ci.yml',
            status: 'completed', conclusion: 'failure', head_sha: SHA,
            ...run,
        },
        jobs: jobs ?? [
            { id: 1, run_id: Number(RUN_ID), name: 'rustfmt', status: 'completed', conclusion: 'success', head_sha: SHA },
            { id: 2, run_id: Number(RUN_ID), name: 'Windows desktop', status: 'completed', conclusion: 'failure', head_sha: SHA },
            { id: 3, run_id: Number(RUN_ID), name: 'macOS desktop', status: 'completed', conclusion: 'cancelled', head_sha: SHA },
            { id: 4, run_id: Number(RUN_ID), name: 'assemble', status: 'completed', conclusion: 'skipped', head_sha: SHA },
        ],
        recoveries: [], reads: 0, output: '',
    };
    const github = {
        currentRepository() { state.reads++; return 'dlivxpr/lyrel'; },
        getRun() { state.reads++; return structuredClone(state.run); },
        listLatestJobs() { state.reads++; return structuredClone(state.jobs); },
        rerunFailed(repo, id) {
            // Model GitHub's failed-only operation, retaining completed successes and original SHA.
            assert.equal(repo, state.run.repository.full_name);
            assert.equal(id, String(state.run.id));
            const selected = state.jobs.filter(job => ['failure', 'cancelled', 'timed_out'].includes(job.conclusion));
            state.recoveries.push({ sha: state.run.head_sha, jobs: selected.map(job => job.name) });
            for (const job of selected) { job.status = 'queued'; job.conclusion = null; }
            state.run.status = 'queued';
        },
    };
    return {
        state,
        invoke: argv => main(argv, { github, env, stdout: { write(text) { state.output += text; } } }),
    };
}

test('dry-run previews only recoverable latest jobs without changing the run', async () => {
    const { state, invoke } = fixture();
    const before = structuredClone({ run: state.run, jobs: state.jobs });
    const result = await invoke([RUN_ID, '--dry-run']);
    assert.equal(result.dryRun, true);
    assert.deepEqual({ run: state.run, jobs: state.jobs }, before);
    assert.deepEqual(state.recoveries, []);
    assert.match(state.output, new RegExp(`Original SHA: ${SHA}`));
    assert.match(state.output, /Windows desktop/);
    assert.match(state.output, /macOS desktop/);
    assert.doesNotMatch(state.output, /rustfmt|assemble/);
});

for (const [name, run] of [
    ['successful', { conclusion: 'success' }],
    ['in progress', { status: 'in_progress' }],
    ['queued', { status: 'queued' }],
    ['unknown conclusion', { conclusion: 'action_required' }],
    ['different repository', { repository: { full_name: 'other/lyrel' } }],
    ['different run ID', { id: 39576001223 }],
    ['unsupported workflow', { name: 'Audit', path: '.github/workflows/audit.yml' }],
    ['name-only workflow impersonation', { path: '.github/workflows/audit.yml' }],
    ['mismatched workflow identity', { name: 'Release' }],
    ['missing original SHA', { head_sha: null }],
]) {
    test(`rejects ${name} runs before scheduling any recovery`, async () => {
        const { state, invoke } = fixture({ run });
        const before = structuredClone({ run: state.run, jobs: state.jobs });
        await assert.rejects(invoke([RUN_ID]));
        assert.deepEqual(state.recoveries, []);
        assert.deepEqual({ run: state.run, jobs: state.jobs }, before);
    });
}

for (const args of [[], ['0'], ['-1'], ['1.5'], ['1;echo token'], [RUN_ID, '--all'], [RUN_ID, '--dry-run', '--dry-run']]) {
    test(`rejects invalid CLI input ${JSON.stringify(args)} without contacting GitHub`, async () => {
        const { state, invoke } = fixture();
        await assert.rejects(invoke(args), /Usage:/);
        assert.equal(state.reads, 0);
        assert.deepEqual(state.recoveries, []);
    });
}

test('rejects a failed run whose latest jobs have already succeeded', async () => {
    const { state, invoke } = fixture({ jobs: [
        { id: 1, run_id: Number(RUN_ID), name: 'Windows desktop', status: 'completed', conclusion: 'success', head_sha: SHA },
    ] });
    await assert.rejects(invoke([RUN_ID]), /no failed or cancelled latest jobs/);
    assert.deepEqual(state.recoveries, []);
});

test('rejects an empty latest-job response rather than rescheduling the entire run', async () => {
    const { state, invoke } = fixture({ jobs: [] });
    await assert.rejects(invoke([RUN_ID]), /no failed or cancelled latest jobs/);
    assert.deepEqual(state.recoveries, []);
});

for (const job of [
    { status: 'in_progress' },
    { run_id: 39576001223 },
    { head_sha: 'b'.repeat(40) },
]) {
    test(`rejects inconsistent latest-job evidence ${JSON.stringify(job)}`, async () => {
        const { state, invoke } = fixture({ jobs: [
            { id: 2, run_id: Number(RUN_ID), name: 'Windows desktop', status: 'completed', conclusion: 'failure', head_sha: SHA, ...job },
        ] });
        await assert.rejects(invoke([RUN_ID]), /Latest jobs/);
        assert.deepEqual(state.recoveries, []);
    });
}

for (const conclusion of ['cancelled', 'timed_out']) {
    test(`recovers completed ${conclusion} Release runs at their original SHA`, async () => {
        const { state, invoke } = fixture({
            run: { name: 'Release', path: '.github/workflows/release.yml', conclusion },
            jobs: [
                { id: 2, run_id: Number(RUN_ID), name: 'Linux server', status: 'completed', conclusion, head_sha: SHA },
            ],
        });
        await invoke([RUN_ID]);
        assert.deepEqual(state.recoveries, [{ sha: SHA, jobs: ['Linux server'] }]);
        assert.equal(state.jobs[0].status, 'queued');
    });
}

test('rejects malformed environment repository identity before any GitHub call', async () => {
    const { state, invoke } = fixture({ env: { GITHUB_REPOSITORY: 'dlivxpr/lyrel/extra' } });
    await assert.rejects(invoke([RUN_ID]));
    assert.equal(state.reads, 0);
    assert.deepEqual(state.recoveries, []);
});

test('propagates a denied GitHub recovery without retrying or reporting success', async () => {
    const denied = Object.assign(new Error('permission denied'), { exitCode: 4 });
    let attempts = 0;
    let output = '';
    const { state } = fixture();
    const github = {
        getRun: () => structuredClone(state.run),
        listLatestJobs: () => structuredClone(state.jobs),
        rerunFailed() { attempts++; throw denied; },
    };
    await assert.rejects(main([RUN_ID], {
        github, env: { GITHUB_REPOSITORY: 'dlivxpr/lyrel' },
        stdout: { write(text) { output += text; } },
    }), error => error === denied);
    assert.equal(attempts, 1);
    assert.equal(state.run.status, 'completed');
    assert.doesNotMatch(output, /Requested failed-only recovery/);
});

test('the real entrypoint asks GitHub to recover failed jobs, not successful jobs or a new HEAD', async t => {
    const { state } = fixture();
    let writes = 0;
    t.mock.method(require('node:child_process'), 'spawnSync', (command, args, options) => {
        assert.equal(command, 'gh');
        assert.equal(options.shell, false);
        const ok = value => ({ status: 0, stdout: JSON.stringify(value), stderr: '' });
        if (args[0] === 'repo' && args[1] === 'view') return ok({ nameWithOwner: 'dlivxpr/lyrel' });
        if (args[0] === 'api') {
            const url = new URL(args[1], 'https://api.github.com/');
            if (url.pathname === `/repos/dlivxpr/lyrel/actions/runs/${RUN_ID}`) return ok(state.run);
            if (url.pathname === `/repos/dlivxpr/lyrel/actions/runs/${RUN_ID}/jobs` && url.searchParams.get('filter') === 'latest') {
                // Multiple API pages must contribute failures to the preview and recovery.
                return ok([{ jobs: state.jobs.slice(0, 2) }, { jobs: state.jobs.slice(2) }]);
            }
            throw new Error(`Unexpected read endpoint: ${url.pathname}`);
        }
        if (args[0] !== 'run' || args[1] !== 'rerun') throw new Error('Only original-run recovery is permitted.');
        if (args[2] !== RUN_ID || args[args.indexOf('--repo') + 1] !== 'dlivxpr/lyrel') throw new Error('Unknown recovery target.');
        writes++;
        const selected = args.includes('--failed')
            ? state.jobs.filter(job => ['failure', 'cancelled', 'timed_out'].includes(job.conclusion))
            : state.jobs;
        for (const job of selected) { job.status = 'queued'; job.conclusion = null; }
        state.run.status = 'queued';
        return ok(null);
    });
    const result = await main([RUN_ID], { env: { GITHUB_SHA: 'b'.repeat(40) }, stdout: { write() {} } });
    assert.equal(writes, 1);
    assert.equal(state.run.head_sha, SHA);
    assert.equal(result.sha, SHA);
    assert.deepEqual(result.jobs.map(job => job.name), ['Windows desktop', 'macOS desktop']);
    assert.deepEqual(state.jobs.map(job => job.status), ['completed', 'queued', 'queued', 'completed']);
    assert.equal(state.jobs[0].conclusion, 'success');
    assert.equal(state.jobs[3].conclusion, 'skipped');
});

test('GitHub process failures retain their exit status and never attempt a write', async t => {
    let calls = 0;
    t.mock.method(require('node:child_process'), 'spawnSync', () => {
        calls++;
        return { status: 4, stdout: '', stderr: 'authentication required' };
    });
    await assert.rejects(main([RUN_ID], { env: { GITHUB_REPOSITORY: 'dlivxpr/lyrel' }, stdout: { write() {} } }),
        error => error.exitCode === 4 && /authentication required/.test(error.message));
    assert.equal(calls, 1);
});
