'use strict';

const { execFileSync } = require('node:child_process');
const path = require('node:path');

const editor = 'lyrel-workbench-editor';
const terminal = 'lyrel-terminal';
// `lyrel-content-diff` is GUI-free but desktop-owned: the tree-sitter
// grammars must never enter the CLI, protocol, or headless production graphs,
// and the shell is its only consumer. It is excluded from the headless build
// with the other desktop-only packages.
const desktopMembers = ['lyrel', 'lyrel-updater', 'lyrel-ui-base', 'lyrel-markdown-editor', terminal, editor, 'lyrel-content-diff'];
const workspaceMembers = new Set([
  'lyrel-protocol', 'lyrel-core', 'lyrel-server', 'lyrel-cli',
  'lyrel-document', 'lyrel-insights', 'lyrel-test-support', ...desktopMembers,
]);
const desktopTargets = ['x86_64-pc-windows-msvc', 'aarch64-apple-darwin'];
const headlessTargets = ['x86_64-unknown-linux-gnu', 'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'];
const editorEngines = ['lyrel-document', 'lyrel-markdown-editor', 'webview2-com', 'wry', 'merman'];
const forbidden = {
  'lyrel-cli': ['lyrel-core', 'russh', 'russh-sftp', 'tokio', 'rusqlite', 'portable-pty'],
  'lyrel-server': ['lyrel-insights'],
  'lyrel-ui-base': ['lyrel', 'lyrel-core', terminal, editor, 'rio-vt', 'rio-graphics', ...editorEngines],
  [terminal]: ['lyrel', editor, ...editorEngines],
  // The Markdown image proxy resolver is injected by the desktop shell, so
  // the workbench links none of the installer's archive machinery.
  [editor]: ['lyrel', terminal, 'async_zip'],
};
const shellEngines = [
  'rusqlite', 'rusty-leveldb', 'aes-gcm', ...editorEngines, 'lsp-types', 'tree-sitter',
];

function graphViolations(packages, normal, headless) {
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
    // Side Panel previews are owned by the desktop shell. Keep their native
    // engines platform-scoped and the draw.io document bridge in the editor.
    if (!rename && name === 'webview2-com' && dependency.target === 'cfg(windows)') continue;
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
  // The workbench editor gets its Markdown image proxy resolver injected by
  // the desktop shell; enabling lyrel-core's installer feature would compile
  // the downloader back into the editor's dependency graph.
  for (const dependency of byName.get(editor).dependencies) {
    if (dependency.name === 'lyrel-core' && dependency.features?.includes('remote-install')) {
      violations.push(`${editor} must not enable the remote-install feature on lyrel-core`);
    }
  }
  for (const [name, denied] of Object.entries(forbidden)) {
    const dependencies = normal.get(name);
    if (!dependencies) throw Error(`Missing resolved production graph: ${name}`);
    for (const dependency of denied) {
      if (dependencies.has(dependency)) violations.push(`${name} reaches ${dependency} through normal dependencies`);
    }
  }
  for (const name of desktopMembers) {
    if (headless.has(name)) violations.push(`Linux headless production graph reaches ${name}`);
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
  const normalNames = (targets, ...selection) => {
    const tree = cargo('tree', '--locked', '--all-features', ...targets.flatMap(target => ['--target', target]),
      ...selection, '--edges', 'normal', '--prefix', 'none', '--format', '{p}', '--color', 'never');
    return new Set(tree.trim().split(/\r?\n/).map(line => line.split(' ', 1)[0]));
  };
  // metadata keeps dev/build edges and target-qualified declarations for the
  // direct checks. cargo tree selects a package's resolved normal edges, so a
  // dev-only fixture doesn't masquerade as part of a shipped binary.
  const normal = new Map(Object.keys(forbidden).map((name) => {
    const targets = name === 'lyrel-cli' || name === 'lyrel-server' ? [...desktopTargets, ...headlessTargets] : desktopTargets;
    const names = normalNames(targets, '-p', name);
    if (!names.delete(name)) throw Error(`Cargo did not resolve the package ${name}`);
    return [name, names];
  }));
  // --exclude removes workspace roots, not dependencies reached from them.
  const headless = normalNames(headlessTargets, '--workspace', ...desktopMembers.flatMap(name => ['--exclude', name]));
  const violations = graphViolations(packages, normal, headless);
  if (violations.length) {
    console.error(violations.join('\n'));
    process.exitCode = 1;
    return;
  }
  console.log('Workspace production dependency constraints pass');
}

if (require.main === module) main();
module.exports = { graphViolations };
