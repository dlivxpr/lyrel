'use strict';

const { execFileSync } = require('node:child_process');
const { appendFileSync, readFileSync } = require('node:fs');

const FULL = { desktop: true, linux: true, server: true, release: true };
const NONE = { desktop: false, linux: false, server: false, release: false };
const SHA = /^[0-9a-f]{40}$/;
// 显式要求完整门禁的标签：Draft PR 打上它之后同样跑三平台矩阵。
const FULL_GATE_LABEL = 'ci:full';
const releaseOnly = new Set([
  '.github/workflows/release.yml',
  '.github/workflows/release-build.yml',
  '.github/scripts/assemble-release.sh',
  '.github/scripts/bundle-macos.sh',
  '.github/scripts/bundle-windows.ps1',
  '.github/scripts/verify-windows-package.ps1',
  '.github/scripts/windows-installer.iss',
  '.github/scripts/generate-checksums.sh',
]);
const sharedReleaseInputs = new Set([
  '.github/scripts/assert-static.sh',
  '.github/scripts/assert-macho.sh',
  '.github/scripts/assert-no-vcruntime.ps1',
  '.github/scripts/select-macos-sdk.sh',
  '.github/scripts/server-artifact.cjs',
  '.github/scripts/ci-server-reuse.cjs',
]);

function git(...args) {
  return execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

function readEvent(env) {
  try {
    return JSON.parse(readFileSync(env.GITHUB_EVENT_PATH, 'utf8'));
  } catch {
    // 事件体是门禁输入之一：读不到就走保守路径，由 gate() 退回完整矩阵。
    return undefined;
  }
}

function changedFiles(env) {
  const event = JSON.parse(readFileSync(env.GITHUB_EVENT_PATH, 'utf8'));
  const after = env.GITHUB_SHA;
  if (!SHA.test(after) || git('rev-parse', 'HEAD').trim() !== after) {
    throw new Error('checkout does not match the event SHA');
  }
  let before;
  if (env.GITHUB_EVENT_NAME === 'push') {
    before = event.before;
    if (event.forced || event.after !== after || !SHA.test(before) || /^0+$/.test(before)) {
      throw new Error('push range is incomplete or rewritten');
    }
    git('merge-base', '--is-ancestor', before, after);
  } else if (env.GITHUB_EVENT_NAME === 'pull_request') {
    before = event.pull_request?.base?.sha;
    const head = event.pull_request?.head?.sha;
    if (!SHA.test(before) || !SHA.test(head)) throw new Error('PR range is incomplete');
    const parents = git('show', '-s', '--format=%P', after).trim().split(' ');
    if (parents.length !== 2 || parents[0] !== before || parents[1] !== head) {
      throw new Error('checkout is not the expected PR merge commit');
    }
  } else {
    throw new Error('manual or unknown event requires full validation');
  }
  // NUL separators preserve unusual filenames; disabling renames includes both paths.
  return git('diff', '--name-only', '--no-renames', '-z', before, after, '--').split('\0').filter(Boolean);
}

// 不进入编译或打包的输入：文档站内容、仓库代理说明、issue 模板、发行说明式
// crates/*/LICENSE.md、自动化脚本自身的单测与分类器。任何不在此列的未知路径
// 都落到下面的保守分支，绝不因为「看起来像文档」而跳过编译。
function isPreflightOnly(file) {
  return (
    /^docs\/.*\.(mdx?|json|css|html|svg|ico)$/.test(file) ||
    /^[^/]+\.md$/.test(file) ||
    /^\.agent\//.test(file) ||
    /^\.github\/ISSUE_TEMPLATE\//.test(file) ||
    /^crates\/[^/]+\/[A-Z_]+\.md$/.test(file) ||
    /^\.github\/scripts\/[^/]+\.test\.cjs$/.test(file) ||
    file === '.github/scripts/ci-changes.cjs' ||
    file === '.github/scripts/ci-main-reuse.cjs' ||
    file === '.github/scripts/host-boundary.cjs' ||
    file === '.github/scripts/check-host-boundary.sh' ||
    file === '.github/dependabot.yml' ||
    file === '.gitattributes' ||
    file === '.gitignore'
  );
}

// 只在桌面（GUI/updater/native harness）与 Apple/Windows 目标上编译的输入；
// Linux headless 显式排除桌面包；manifest 变化仍按共享解析输入处理。
function isDesktopOnly(file) {
  return (
    file.startsWith('src/') ||
    file === 'build.rs' ||
    file.startsWith('crates/lyrel-markdown-editor/') ||
    file.startsWith('crates/lyrel-updater/') ||
    file.startsWith('crates/lyrel-ui-base/') ||
    file.startsWith('crates/lyrel-terminal/') ||
    file.startsWith('crates/lyrel-workbench-editor/') ||
    file.startsWith('crates/lyrel-content-diff/') ||
    file.startsWith('tests/') ||
    file.startsWith('assets/icons/') ||
    file.startsWith('assets/drawio/') ||
    file.startsWith('assets/themes/')
  );
}

// GUI-free 也进 Linux headless 图的成员：CLI、lyrel-document 的全部文档引擎、
// lyrel-insights 的 skills/statistics/github/usage 模型（浏览器读取器仍只进
// 桌面目标）、共享测试夹具以及集成测试。lyrel-core 与 lyrel-protocol 同是
// server 生产依赖，源码不在此列：保守分支同时触发 server。
function isDesktopAndLinuxOnly(file) {
  return (
    file.startsWith('crates/lyrel-cli/') ||
    file.startsWith('crates/lyrel-document/') ||
    file.startsWith('crates/lyrel-insights/') ||
    file.startsWith('crates/lyrel-test-support/') ||
    /^crates\/lyrel-(core|server)\/tests\//.test(file)
  );
}

function classify(files) {
  const scopes = { ...NONE };
  if (files.length === 0) return { ...FULL };
  for (const file of files) {
    // LICENSE and arbitrary Markdown are build inputs, not documentation by suffix.
    if (isPreflightOnly(file)) continue;
    if (/(^|\/)Cargo\.(toml|lock)$|^\.cargo\//.test(file)) {
      scopes.desktop = scopes.linux = scopes.server = true;
    } else if (releaseOnly.has(file)) {
      scopes.release = true;
    } else if (isDesktopOnly(file)) {
      scopes.desktop = true;
    } else if (isDesktopAndLinuxOnly(file)) {
      scopes.desktop = scopes.linux = true;
    } else {
      scopes.desktop = scopes.linux = scopes.server = true;
      if (sharedReleaseInputs.has(file)) scopes.release = true;
    }
  }
  return scopes;
}

// 事件层门禁：这次运行要不要三平台完整矩阵、要不要打包发行产物。
// 缺字段一律按完整门禁处理；只有能证明是 Draft（且没有 ci:full 标签）才降级。
function gate(env, event) {
  if (env.GITHUB_EVENT_NAME === 'workflow_dispatch') {
    // 手动触发的默认 scope 是 ci：验证与 PR 一致，但不额外跑发行打包。
    const production = event?.inputs?.scope === 'full';
    return { tier: 'full', packaging: production, production };
  }
  if (env.GITHUB_EVENT_NAME === 'pull_request') {
    if (event?.action === 'labeled' && event.label?.name !== FULL_GATE_LABEL) {
      // 无关标签不改变代码：只留 preflight 作为状态信号，也不取消在跑的完整门禁。
      return { tier: 'fast', skip: true, production: false };
    }
    const request = event?.pull_request ?? {};
    const full = Array.isArray(request.labels) && request.labels.some(label => label?.name === FULL_GATE_LABEL);
    return { tier: request.draft === true && !full ? 'fast' : 'full', production: false };
  }
  return { tier: 'full', production: true };
}

function matrixFor(scopes, tier) {
  const include = [];
  // Draft 的快速层只要 Windows + Linux；macOS 槽位留给完整门禁与 main。
  if (scopes.desktop && tier === 'full') include.push({ runner: 'macos-15', target: 'aarch64-apple-darwin' });
  if (scopes.desktop) include.push({ runner: 'windows-latest', target: 'x86_64-pc-windows-msvc' });
  if (scopes.linux) include.push({ runner: 'ubuntu-24.04', target: 'x86_64-unknown-linux-gnu' });
  return { include };
}

function main(env = process.env) {
  const event = readEvent(env);
  const decision = gate(env, event);
  let scopes;
  if (decision.skip) {
    scopes = { ...NONE };
  } else {
    try {
      scopes = classify(changedFiles(env));
    } catch (error) {
      console.error(`Conservative full validation: ${error.message}`);
      scopes = { ...FULL };
    }
  }
  // 快速层与手动 ci scope 都不产生需要上传的发行产物。
  if (decision.tier !== 'full') scopes = { ...scopes, server: false, release: false };
  if (decision.packaging === false) scopes = { ...scopes, release: false };
  const result = { ...scopes, tier: decision.tier, production: decision.production, matrix: matrixFor(scopes, decision.tier) };
  if (env.GITHUB_OUTPUT) {
    // 字符串（tier）要原样输出，布尔与 matrix 才用 JSON，否则下游比较会带上引号。
    const encode = value => (typeof value === 'string' ? value : JSON.stringify(value));
    appendFileSync(env.GITHUB_OUTPUT, Object.entries(result)
      .map(([key, value]) => `${key}=${encode(value)}\n`).join(''));
  }
  console.log(JSON.stringify(result));
}

if (require.main === module) main();

module.exports = { FULL_GATE_LABEL, classify, gate, isPreflightOnly, matrixFor };
