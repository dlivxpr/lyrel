#!/usr/bin/env node
'use strict';

const childProcess = require('node:child_process');

const FAILED = new Set(['failure', 'cancelled', 'timed_out']);
const WORKFLOWS = new Map([
    ['.github/workflows/ci.yml', 'CI'],
    ['.github/workflows/release.yml', 'Release'],
]);

function githubAdapter(env) {
    function gh(args, json = true) {
        const result = childProcess.spawnSync('gh', args, { encoding: 'utf8', env, shell: false, maxBuffer: 16 * 1024 * 1024 });
        if (result.error) throw result.error;
        if (result.status !== 0) {
            const error = new Error(`gh failed (${result.status ?? result.signal}): ${result.stderr.trim()}`);
            error.exitCode = result.status || 1;
            throw error;
        }
        return json ? JSON.parse(result.stdout) : result.stdout;
    }
    return {
        currentRepository: () => gh(['repo', 'view', '--json', 'nameWithOwner']).nameWithOwner,
        getRun: (repo, id) => gh(['api', `repos/${repo}/actions/runs/${id}`]),
        listLatestJobs: (repo, id) => gh([
            'api', `repos/${repo}/actions/runs/${id}/jobs?filter=latest&per_page=100`, '--paginate', '--slurp',
        ]).flatMap(page => page.jobs),
        rerunFailed: (repo, id) => gh(['run', 'rerun', id, '--repo', repo, '--failed'], false),
    };
}

async function main(argv = process.argv.slice(2), { env = process.env, github = githubAdapter(env), stdout = process.stdout } = {}) {
    if (argv.length < 1 || argv.length > 2 || !/^[1-9]\d*$/.test(argv[0]) || (argv.length === 2 && argv[1] !== '--dry-run')) {
        throw new Error('Usage: node scripts/rerun-failed-ci.cjs <run-id> [--dry-run]');
    }
    const id = argv[0];
    const dryRun = argv[1] === '--dry-run';
    const repo = env.GITHUB_REPOSITORY || await github.currentRepository();
    if (typeof repo !== 'string' || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) {
        throw new Error('Cannot determine a valid current OWNER/REPO.');
    }
    const run = await github.getRun(repo, id);
    if (!run || String(run.id) !== id || run.repository?.full_name?.toLowerCase() !== repo.toLowerCase()) {
        throw new Error('Run does not belong to the requested current repository.');
    }
    if (!WORKFLOWS.has(run.path) || WORKFLOWS.get(run.path) !== run.name) {
        throw new Error('Only the current repository CI and Release workflows can be recovered.');
    }
    if (run.status !== 'completed' || !FAILED.has(run.conclusion)) {
        throw new Error('Run must be completed with failure, cancelled, or timed_out conclusion.');
    }
    if (typeof run.head_sha !== 'string' || !/^[a-fA-F0-9]{40}$/.test(run.head_sha)) {
        throw new Error('Run has no valid original head SHA.');
    }
    const allJobs = await github.listLatestJobs(repo, id);
    if (!Array.isArray(allJobs) || allJobs.some(job => !job || String(job.run_id) !== id || job.status !== 'completed' || job.head_sha !== run.head_sha)) {
        throw new Error('Latest jobs must be completed and belong to this run and its original SHA.');
    }
    const jobs = allJobs.filter(job => FAILED.has(job.conclusion));
    if (!jobs.length) throw new Error('Run has no failed or cancelled latest jobs to recover.');
    stdout.write(`${dryRun ? 'Preview' : 'Recovery'}: ${repo} run ${id}\nOriginal SHA: ${run.head_sha}\nFailed jobs:\n`);
    for (const job of jobs) stdout.write(`  ${job.name} (${job.conclusion}, job ${job.id})\n`);
    if (!dryRun) {
        await github.rerunFailed(repo, id);
        stdout.write('Requested failed-only recovery of the original run; successful jobs are not rerun.\n');
    }
    return { repo, runId: id, sha: run.head_sha, jobs, dryRun };
}

module.exports = { main };

if (require.main === module) {
    main().catch(error => {
        let message = error.message;
        for (const key of ['GH_TOKEN', 'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN']) {
            if (process.env[key]) message = message.split(process.env[key]).join('[REDACTED]');
        }
        process.stderr.write(`${message}\n`);
        process.exitCode = error.exitCode || 1;
    });
}
