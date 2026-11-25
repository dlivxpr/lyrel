'use strict';

// 判断一次 main push 能否复用已完成的 PR 门禁，跳过后台重复的桌面/Linux 测试。
//
// 证据来自 GitHub 的 run/job 结论、commit-associated PR 及 reusable workflow
// 的不可变解析 SHA。合并后 run.pull_requests 可能为空，临时 merge ref 也会删除，
// 两者都不能作为持久证据。按精确 repo/path/ref 定位当次测试 merge SHA，再从
// 同仓库 git.getCommit 核对双 parents、tree、main 祖先与门禁定义；不读取 artifact，
// 不把 PR.merge_commit_sha（squash 结果）或源头 head_commit.tree_id 当测试树。
// 三个平台与整个 run 必须成功，证据时效只限制复用窗口，不声称 runner 镜像不变。
//
// 任何一项缺失、过期或含义不明都返回 reuse: false，调用方跑完整集合。
// 这里只有 fail closed 一种默认。

const { execFileSync } = require('node:child_process');

const WORKFLOW_PATH = '.github/workflows/ci.yml';
// PR 可以改工作流，所以只接受与已测试 base 相同的门禁定义。
const GATE_FILES = [WORKFLOW_PATH, '.github/workflows/release-build.yml'];
const PLATFORM_JOBS = [
  'build & test (aarch64-apple-darwin)',
  'build & test (x86_64-pc-windows-msvc)',
  'build & test (x86_64-unknown-linux-gnu)',
];
const REQUIRED_JOBS = ['preflight', ...PLATFORM_JOBS];
const RELEASE_JOBS = [
  'build (x86_64-pc-windows-msvc)',
  'build (aarch64-apple-darwin)',
  'pack (x86_64-pc-windows-msvc)',
  'pack (aarch64-apple-darwin)',
  'server-musl (x86_64-unknown-linux-musl, lyrel-server-linux-x86_64-musl)',
  'server-musl (aarch64-unknown-linux-musl, lyrel-server-linux-aarch64-musl)',
  'server-macos (aarch64-apple-darwin, arm64, lyrel-server-macos-aarch64)',
  'server-windows (x86_64-pc-windows-msvc, lyrel-server-windows-x86_64.zip)',
].map(name => `release-validation / ${name}`);
// 有界复用近期完整门禁；这不是 runner 镜像或工具链版本相同的证明。
const EVIDENCE_MAX_AGE_MS = 72 * 60 * 60 * 1000;
const CLOCK_SKEW_MS = 5 * 60 * 1000;
const MAX_CANDIDATE_RUNS = 25;
const SHA = /^[0-9a-f]{40}$/;

function requireValue(condition, reason) {
  if (!condition) throw Error(reason);
}

function positiveId(value) {
  return /^[1-9][0-9]*$/.test(String(value)) && Number.isSafeInteger(Number(value));
}

function gitRunner(cwd) {
  return (...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

// run 本身的身份：仓库、workflow、事件、结论、PR 绑定、新鲜度。
function verifyRunIdentity({ repository, workflow, run, event = 'pull_request', now = Date.now(), maxAgeMs = EVIDENCE_MAX_AGE_MS }) {
  requireValue(workflow?.path === WORKFLOW_PATH && positiveId(workflow?.id), 'unexpected workflow identity');
  const origin = run?.repository ?? {};
  const head = run?.head_repository ?? {};
  requireValue(
    origin.full_name === repository && origin.fork === false &&
      head.full_name === repository && head.fork === false &&
      positiveId(origin.id) && head.id === origin.id,
    'CI run does not come from this repository',
  );
  requireValue(
    positiveId(run.id) && positiveId(run.run_attempt) &&
      run.workflow_id === workflow.id && run.path === WORKFLOW_PATH,
    'unexpected CI run identity',
  );
  requireValue(
    run.event === event && run.status === 'completed' && run.conclusion === 'success',
    `CI run is not a completed ${event} success`,
  );
  const created = Date.parse(run.created_at);
  requireValue(
    Number.isFinite(created) && now - created <= maxAgeMs && created - now <= CLOCK_SKEW_MS,
    'CI evidence is outside the freshness window',
  );
}

function testedMergeCommit({ repository, run, request }) {
  requireValue(
    positiveId(request?.number) && request.state === 'closed' &&
      Number.isFinite(Date.parse(request.merged_at)) && request.base?.ref === 'main' &&
      request.head?.sha === run.head_sha && SHA.test(run.head_sha ?? '') &&
      request.base?.repo?.id === run.repository.id &&
      request.head?.repo?.id === run.repository.id && SHA.test(request.base?.sha ?? ''),
    'CI run is not bound to the tested merged pull request',
  );
  const references = (run.referenced_workflows ?? []).filter(reference =>
    SHA.test(reference.sha ?? '') &&
    reference.ref === `refs/pull/${request.number}/merge` &&
    reference.path === `${repository}/.github/workflows/release-build.yml@${reference.sha}`);
  requireValue(references.length === 1, 'missing unique immutable tested workflow commit');
  return references[0].sha;
}

// 每个 job 都必须完成；允许 skipped（矩阵按 scope 裁剪时的正常状态），
// 但 preflight 与三个平台的 build & test 必须各出现一次且成功。
function verifyJobs({ run, jobs, required = REQUIRED_JOBS }) {
  requireValue(Array.isArray(jobs) && jobs.length > 0, 'CI run has no job records');
  for (const job of jobs) {
    requireValue(
      job.status === 'completed' && (job.conclusion === 'success' || job.conclusion === 'skipped'),
      `CI job did not finish cleanly: ${job.name}`,
    );
    requireValue(
      job.head_sha === run.head_sha && job.run_attempt === run.run_attempt,
      `CI job is from another attempt: ${job.name}`,
    );
  }
  for (const name of required) {
    const matches = jobs.filter(job => job.name === name);
    requireValue(matches.length === 1 && matches[0].conclusion === 'success', `missing successful gate job: ${name}`);
  }
}

function completedJobs(jobs, names) {
  return names.every(name => {
    const matches = jobs.filter(job => job.name === name);
    return matches.length === 1 && matches[0].conclusion === 'success';
  });
}

// 相同 tree、双 parents 与相同门禁都必须由不可变 commit 对象证明。
function verifyMerge({ sourceTree, gate, repository, run, request, merge }) {
  requireValue(
    merge?.commit === testedMergeCommit({ repository, run, request }) &&
      merge.base === request.base.sha && merge.head === run.head_sha,
    'immutable merge commit does not belong to the tested pull request',
  );
  requireValue(SHA.test(merge.tree ?? '') && merge.tree === sourceTree, 'main tree differs from the tested tree');
  for (const file of GATE_FILES) {
    requireValue(
      SHA.test(gate?.[file] ?? '') && gate[file] === merge.baseGate?.[file],
      `gate definition changed since the pull request ran: ${file}`,
    );
  }
}

function verifyRun({ repository, sourceTree, gate, workflow, run, request, jobs, merge, now, maxAgeMs }) {
  verifyRunIdentity({ repository, workflow, run, now, maxAgeMs });
  verifyJobs({ run, jobs });
  verifyMerge({ sourceTree, gate, repository, run, request, merge });
  return { runId: String(run.id), runAttempt: String(run.run_attempt), pullRequest: request.number };
}

// REST 按 GitHub 记录的解析 SHA 读取 merge 对象，不重新抓取临时 refs/pull/N/merge。
async function readMerge({ github, repo, git, commit, sourceGate }) {
  requireValue(SHA.test(commit ?? ''), 'tested merge SHA is missing');
  const object = (await github.rest.git.getCommit({ ...repo, commit_sha: commit })).data;
  requireValue(object.sha === commit && object.parents?.length === 2, 'tested commit is not the expected two-parent merge');
  const [base, head] = object.parents.map(parent => parent.sha);
  requireValue(SHA.test(base ?? '') && SHA.test(head ?? '') && SHA.test(object.tree?.sha ?? ''), 'incomplete merge object');
  git('merge-base', '--is-ancestor', base, 'HEAD');
  const baseGate = {};
  for (const file of GATE_FILES) {
    // 文件首部注释与门禁无关。保留 YAML 正文的每个字节，包括块字符串。
    try {
      const blob = git('rev-parse', `${base}:${file}`).trim();
      const source = sourceGate?.[file];
      const withoutComments = text => text.replace(/^(?:#[^\r\n]*\r?\n)+/, '');
      baseGate[file] = source && blob !== source &&
        withoutComments(git('cat-file', 'blob', blob)) === withoutComments(git('cat-file', 'blob', source))
        ? source : blob;
    } catch (error) {
      throw Error(`gate definition missing at the tested base: ${file}`);
    }
  }
  return { commit, base, head, tree: object.tree.sha, baseGate };
}

async function verifyParentGate({ github, repo, workflow, parent, now, maxAgeMs }) {
  const { workflow_runs: [run] } = (await github.rest.actions.listWorkflowRuns({
    ...repo, workflow_id: workflow.id, event: 'push', branch: 'main', head_sha: parent, per_page: 1,
  })).data;
  requireValue(run?.head_sha === parent && run.head_branch === 'main', 'no CI run for the immediate main parent');
  verifyRunIdentity({ repository: `${repo.owner}/${repo.repo}`, workflow, run, event: 'push', now, maxAgeMs });
  const jobs = await github.paginate(github.rest.actions.listJobsForWorkflowRun, {
    ...repo, run_id: run.id, filter: 'latest', per_page: 100,
  });
  verifyJobs({ run, jobs });
  for (const name of PLATFORM_JOBS) {
    const job = jobs.find(entry => entry.name === name);
    const testStep = name.endsWith('x86_64-unknown-linux-gnu)') ? 'Test Linux headless' : 'Test desktop library and headless';
    requireValue(job.steps?.some(step => step.name === testStep &&
      step.status === 'completed' && step.conclusion === 'success'), `parent tests were not executed: ${name}`);
  }
  return String(run.id);
}

async function discoverComparableRun({
  github, repo, sourceSha, versionParent, log = console.log, now = Date.now(),
  git = gitRunner(process.env.GITHUB_WORKSPACE || process.cwd()), maxAgeMs = EVIDENCE_MAX_AGE_MS,
}) {
  try {
    requireValue(SHA.test(sourceSha ?? ''), 'source SHA is not immutable');
    requireValue(git('rev-parse', 'HEAD').trim() === sourceSha, 'checkout does not match the event SHA');
    const target = versionParent || sourceSha;
    if (versionParent) {
      requireValue(SHA.test(versionParent) && git('show', '-s', '--format=%P', sourceSha).trim() === versionParent,
        'version evidence must target the immediate single parent');
    }
    const sourceTree = git('rev-parse', `${target}^{tree}`).trim();
    const gate = {};
    for (const file of GATE_FILES) gate[file] = git('rev-parse', `${target}:${file}`).trim();
    const workflow = (await github.rest.actions.getWorkflow({ ...repo, workflow_id: 'ci.yml' })).data;
    if (versionParent) {
      try {
        const runId = await verifyParentGate({ github, repo, workflow, parent: versionParent, now, maxAgeMs });
        log(`Using completed main test gate ${runId} for the version-only push`);
        return { reuse: true, versionReuse: true, runId };
      } catch (error) {
        log(`No complete main parent gate: ${error.message}`);
      }
    }
    const runs = (await github.rest.actions.listWorkflowRuns({
      ...repo, workflow_id: workflow.id, event: 'pull_request', status: 'success', per_page: MAX_CANDIDATE_RUNS,
    })).data.workflow_runs ?? [];
    const associatedRequests = sha => github.paginate(github.rest.repos.listPullRequestsAssociatedWithCommit, {
      ...repo, commit_sha: sha, per_page: 100,
    });
    const squashedRequests = (await associatedRequests(target))
      .filter(request => request.merge_commit_sha === target);
    for (const run of runs) {
      try {
        verifyRunIdentity({ repository: `${repo.owner}/${repo.repo}`, workflow, run, now, maxAgeMs });
        const associatedWithRun = requests => requests.filter(request => {
          try {
            testedMergeCommit({ repository: `${repo.owner}/${repo.repo}`, run, request });
            return true;
          } catch { return false; }
        });
        const squashed = associatedWithRun(squashedRequests);
        const candidates = squashed.length > 0 ? squashed : associatedWithRun(await associatedRequests(run.head_sha));
        requireValue(candidates.length === 1, 'missing unique merged pull request association');
        const request = candidates[0];
        const jobs = await github.paginate(github.rest.actions.listJobsForWorkflowRun, {
          ...repo, run_id: run.id, filter: 'latest', per_page: 100,
        });
        verifyJobs({ run, jobs, required: ['preflight'] });
        const reuse = completedJobs(jobs, PLATFORM_JOBS);
        const releaseReuse = !versionParent && completedJobs(jobs, RELEASE_JOBS);
        requireValue(reuse || releaseReuse, 'no complete test or release gate');
        const commit = testedMergeCommit({ repository: `${repo.owner}/${repo.repo}`, run, request });
        const merge = await readMerge({ github, repo, git, commit, sourceGate: gate });
        verifyMerge({ repository: `${repo.owner}/${repo.repo}`, sourceTree, gate, run, request, merge });
        const accepted = { runId: String(run.id), runAttempt: String(run.run_attempt), pullRequest: request.number };
        log(`Reusing completed pull request gate ${accepted.runId} (pull request #${accepted.pullRequest}) for the same tree`);
        if (versionParent) return { reuse: true, versionReuse: true, ...accepted };
        return { reuse, releaseReuse, ...accepted, tree: sourceTree };
      } catch (error) {
        log(`skip CI run ${run.id}: ${error.message}`);
      }
    }
    log('No comparable completed pull request gate; running the full suite');
  } catch (error) {
    log(`tree reuse unavailable; running the full suite: ${error.message}`);
  }
  return { reuse: false };
}

module.exports = {
  EVIDENCE_MAX_AGE_MS, GATE_FILES, PLATFORM_JOBS, REQUIRED_JOBS,
  discoverComparableRun, readMerge, verifyJobs, verifyMerge, verifyRun, verifyRunIdentity,
};
