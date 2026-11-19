'use strict';

const { execFileSync } = require('node:child_process');
const path = require('node:path');

const editor = 'lyrel-workbench-editor';
const terminal = 'lyrel-terminal';
const workspaceMembers = new Set([
  'lyrel-protocol', 'lyrel-core', 'lyrel-server', 'lyrel-cli',
  'lyrel-document', 'lyrel-insights',
  'lyrel', 'lyrel-updater', 'lyrel-ui-base', 'lyrel-markdown-editor', terminal, editor,
  'lyrel-test-support',
]);
const desktopTargets = ['x86_64-pc-windows-msvc', 'aarch64-apple-darwin'];
const headlessTargets = ['x86_64-unknown-linux-gnu', 'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'];
const editorEngines = ['lyrel-document', 'lyrel-markdown-editor', 'webview2-com', 'wry', 'merman'];
const forbidden = {
  'lyrel-cli': ['lyrel-core', 'russh', 'russh-sftp', 'tokio', 'rusqlite', 'portable-pty'],
  'lyrel-server': ['lyrel-insights'],
  'lyrel-ui-base': ['lyrel', 'lyrel-core', terminal, editor, 'rio-vt', 'rio-graphics', ...editorEngines],
  [terminal]: ['lyrel', editor, ...editorEngines],
  [editor]: ['lyrel', terminal],
};
const shellEngines = [
  'rusqlite', 'rusty-leveldb', 'aes-gcm', ...editorEngines, 'lsp-types', 'tree-sitter',
];

function graphViolations(packages, normal) {
  const byName = new Map(packages.map(pkg => [pkg.name, pkg]));
  const missing = [...workspaceMembers].filter(name => !byName.has(name));
  const unexpected = [...byName.keys()].filter(name => !workspaceMembers.has(name));
  const violations = [
    ...missing.map(name => `Missing workspace member: ${name}`),
    ...unexpected.map(name => `Unexpected workspace member: ${name}`),
  ];
  if (missing.length > 0) return violations;
  const directNormal = (name) => new Set(byName.get(name).dependencies.filter(dep => dep.kind === null).map(dep => dep.name));
  for (const { name, dependencies } of packages) {
    if (dependencies.some(dep => dep.name === 'lyrel-test-support' && dep.kind !== 'dev')) {
      violations.push(`${name} declares a non-dev dependency on lyrel-test-support`);
    }
  }
  // Check declarations independently of the selected Cargo target matrix.
  const cli = directNormal('lyrel-cli');
  for (const dependency of forbidden['lyrel-cli']) {
    if (cli.has(dependency)) violations.push(`lyrel-cli declares a direct production dependency on ${dependency}`);
  }
  const server = directNormal('lyrel-server');
  if (server.size !== 1 || !server.has('lyrel-core')) {
    violations.push(`lyrel-server must depend only on lyrel-core in production; found: ${[...server].join(', ') || '(none)'}`);
  }
  const { features: shellFeatures, dependencies: shellDependencies } = byName.get('lyrel');
  const nativeHarness = 'native-markdown-tests';
  for (const dependency of shellDependencies) {
    const { name, kind, rename, optional } = dependency;
    if (kind !== null || !shellEngines.includes(name)) continue;
    // The main-thread Markdown consumer compiles in the shell library to
    // exercise real window composition, but only in the opt-in test build.
    const testOnly = name === 'lyrel-markdown-editor' && optional && !rename &&
      shellFeatures[nativeHarness]?.includes(`dep:${name}`) &&
      !Object.entries(shellFeatures).some(([feature, enables]) => feature !== nativeHarness &&
        enables.some(entry => entry === nativeHarness || entry === `dep:${name}` ||
          entry === name || entry.startsWith(`${name}/`)));
    if (!testOnly) violations.push(`lyrel declares a direct production dependency on ${name}`);
  }
  if (Object.hasOwn(byName.get('lyrel').features, 'updater')) violations.push('lyrel declares the removed updater feature');
  // The pinned GPUI Kit depends on LSP value types. Only the workbench may
  // declare that dependency itself; it owns the actual LSP client.
  for (const name of ['lyrel-ui-base', terminal]) {
    if (directNormal(name).has('lsp-types')) violations.push(`${name} declares a direct production dependency on lsp-types`);
  }
  for (const [from, to] of [[terminal, editor], [editor, terminal]]) {
    if (byName.get(from).dependencies.some(dep => dep.name === to)) violations.push(`${from} declares a direct dependency on ${to}`);
  }
  for (const [name, denied] of Object.entries(forbidden)) {
    const dependencies = normal.get(name);
    if (!dependencies) throw Error(`Missing resolved production graph: ${name}`);
    for (const dependency of denied) {
      if (dependencies.has(dependency)) violations.push(`${name} reaches ${dependency} through normal dependencies`);
    }
  }
  return violations;
}

function main() {
  const cargo = (...args) => execFileSync('cargo', args, {
    cwd: path.resolve(__dirname, '../..'),
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  const { packages } = JSON.parse(cargo('metadata', '--locked', '--format-version', '1', '--no-deps'));
  // metadata keeps dev/build edges and target-qualified declarations for the
  // direct checks. cargo tree selects a package's resolved normal edges, so a
  // dev-only fixture doesn't masquerade as part of a shipped binary.
  const normal = new Map(Object.keys(forbidden).map((name) => {
    const targets = name === 'lyrel-cli' || name === 'lyrel-server' ? [...desktopTargets, ...headlessTargets] : desktopTargets;
    const tree = cargo('tree', '--locked', '--all-features', ...targets.flatMap(target => ['--target', target]), '-p', name,
      '--edges', 'normal', '--prefix', 'none', '--format', '{p}', '--color', 'never');
    const names = new Set(tree.trim().split(/\r?\n/).map(line => line.split(' ', 1)[0]));
    if (!names.delete(name)) throw Error(`Cargo did not resolve the package ${name}`);
    return [name, names];
  }));
  const violations = graphViolations(packages, normal);
  if (violations.length) {
    console.error(violations.join('\n'));
    process.exitCode = 1;
    return;
  }
  console.log('Workspace production dependency constraints pass');
}

if (require.main === module) main();
module.exports = { graphViolations };
