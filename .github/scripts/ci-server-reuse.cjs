'use strict';

const TARGET_JOBS = Object.freeze({
    'x86_64-unknown-linux-musl': 'lyrel-server musl (x86_64-unknown-linux-musl)',
    'aarch64-unknown-linux-musl': 'lyrel-server musl (aarch64-unknown-linux-musl)',
    'aarch64-apple-darwin': 'lyrel-server macOS (aarch64-apple-darwin)'
});
function requireValue(condition, reason) {
    if (!condition) throw Error(reason);
}
function positiveId(value) {
    return /^[1-9][0-9]*$/.test(String(value)) && Number.isSafeInteger(Number(value));
}
function selectServerArtifacts({ repository, sourceSha, workflow, run, jobs, artifacts, now = Date.now() }) {
    requireValue(/^[0-9a-f]{40}$/.test(sourceSha), 'source SHA is not immutable');
    requireValue(workflow.path === '.github/workflows/ci.yml' && positiveId(workflow.id), 'unexpected CI workflow');
    requireValue(run.repository?.full_name === repository && run.head_repository?.full_name === repository &&
        run.head_repository?.fork === false && run.repository?.fork === false &&
        positiveId(run.repository.id) && run.head_repository.id === run.repository.id,
    'CI must originate in the same non-fork repository');
    requireValue(positiveId(run.id) && positiveId(run.run_attempt) && run.workflow_id === workflow.id &&
        run.path === workflow.path && run.event === 'push' && run.head_branch === 'main' &&
        run.head_sha === sourceSha && run.status === 'completed' && run.conclusion === 'success' &&
        Array.isArray(run.pull_requests) && run.pull_requests.length === 0,
    'CI must be a completed successful main push at the exact source SHA');
    for (const name of Object.values(TARGET_JOBS)) {
        const matches = jobs.filter(job => job.name === name);
        requireValue(matches.length === 1 && matches[0].status === 'completed' && matches[0].conclusion === 'success' &&
            matches[0].head_sha === sourceSha && matches[0].run_attempt === run.run_attempt,
        `missing successful current-attempt server job: ${name}`);
    }
    const artifactIds = {};
    for (const target of Object.keys(TARGET_JOBS)) {
        const matches = artifacts.filter(artifact => artifact.name === `ci-server-${target}`);
        requireValue(matches.length === 1, `missing or ambiguous server artifact: ${target}`);
        const artifact = matches[0], origin = artifact.workflow_run;
        requireValue(positiveId(artifact.id) && artifact.expired === false && artifact.size_in_bytes > 0 &&
            Date.parse(artifact.expires_at) > now && origin?.id === run.id && origin.head_sha === sourceSha &&
            origin.repository_id === run.repository.id && origin.head_repository_id === run.repository.id,
        `expired, empty or unbound server artifact: ${target}`);
        artifactIds[target] = String(artifact.id);
    }
    return { runId: String(run.id), runAttempt: String(run.run_attempt), repositoryId: String(run.repository.id), artifactIds };
}
async function discoverServerArtifacts({ github, repo, sourceSha, log = console.log }) {
    const empty = { runId: '', runAttempt: '', repositoryId: '', artifactIds: {} };
    try {
        const workflow = (await github.rest.actions.getWorkflow({ ...repo, workflow_id: 'ci.yml' })).data;
        const runs = await github.paginate(github.rest.actions.listWorkflowRuns, {
            ...repo, workflow_id: workflow.id, head_sha: sourceSha, event: 'push', branch: 'main', status: 'success', per_page: 100
        });
        for (const run of runs) {
            try {
                const jobs = await github.paginate(github.rest.actions.listJobsForWorkflowRun, {
                    ...repo, run_id: run.id, filter: 'latest', per_page: 100
                });
                const artifacts = await github.paginate(github.rest.actions.listWorkflowRunArtifacts, {
                    ...repo, run_id: run.id, per_page: 100
                });
                const result = selectServerArtifacts({ repository: `${repo.owner}/${repo.repo}`, sourceSha, workflow, run, jobs, artifacts });
                log(`Reusing immutable server artifacts from CI run ${result.runId}`);
                return result;
            } catch (error) {
                log(`CI server reuse unavailable for run ${run.id}: ${error.message}`);
            }
        }
        log('No complete trusted CI server artifacts; compiling normally');
    } catch (error) {
        log(`CI server discovery unavailable; compiling normally: ${error.message}`);
    }
    return empty;
}
module.exports = { TARGET_JOBS, selectServerArtifacts, discoverServerArtifacts };
