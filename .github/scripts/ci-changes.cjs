'use strict';

const { execFileSync } = require('node:child_process');
const { appendFileSync, readFileSync } = require('node:fs');

const FULL = { desktop: true, linux: true, server: true, release: true };
const SHA = /^[0-9a-f]{40}$/;
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
  '.github/scripts/server-artifact.cjs',
  '.github/scripts/ci-server-reuse.cjs',
]);

function git(...args) {
  return execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
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

function classify(files) {
  const scopes = { desktop: false, linux: false, server: false, release: false };
  if (files.length === 0) return { ...FULL };
  for (const file of files) {
    // LICENSE and arbitrary Markdown are build inputs, not documentation by suffix.
    if (/^docs\/.*\.mdx?$|^[^/]+\.md$/.test(file)) continue;
    if (/(^|\/)Cargo\.(toml|lock)$/.test(file)) {
      scopes.desktop = scopes.linux = scopes.server = true;
    } else if (releaseOnly.has(file)) {
      scopes.release = true;
    } else if (file.startsWith('src/') || file === 'build.rs') {
      scopes.desktop = true;
    } else if (file.startsWith('crates/lyrel-cli/') || /^crates\/lyrel-(core|server)\/tests\//.test(file)) {
      scopes.desktop = scopes.linux = true;
    } else {
      scopes.desktop = scopes.linux = scopes.server = true;
      if (sharedReleaseInputs.has(file)) scopes.release = true;
    }
  }
  return scopes;
}

function main(env = process.env) {
  let scopes;
  try {
    scopes = classify(changedFiles(env));
  } catch (error) {
    console.error(`Conservative full validation: ${error.message}`);
    scopes = { ...FULL };
  }
  const include = [];
  if (scopes.desktop) include.push(
    { runner: 'macos-15', target: 'aarch64-apple-darwin' },
    { runner: 'windows-latest', target: 'x86_64-pc-windows-msvc' },
  );
  if (scopes.linux) include.push({ runner: 'ubuntu-24.04', target: 'x86_64-unknown-linux-gnu' });
  const result = { ...scopes, matrix: { include } };
  if (env.GITHUB_OUTPUT) {
    appendFileSync(env.GITHUB_OUTPUT, Object.entries(result)
      .map(([key, value]) => `${key}=${JSON.stringify(value)}\n`).join(''));
  }
  console.log(JSON.stringify(result));
}

if (require.main === module) main();
