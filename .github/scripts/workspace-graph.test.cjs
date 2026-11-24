'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');
const { graphViolations } = require('./workspace-graph.cjs');

// Cargo metadata uses package names here, including for renamed dependencies.
const dep = (name, kind = null, rename = null) => ({ name, kind, rename });

function graph() {
  return {
    packages: [
      { name: 'lyrel', features: {}, dependencies: [dep('rusqlite', 'dev'), dep('lyrel-updater')] },
      { name: 'lyrel-server', dependencies: [dep('lyrel-core'), dep('lyrel-insights', 'dev')] },
      { name: 'lyrel-cli', dependencies: [dep('lyrel-protocol'), dep('lyrel-core', 'dev')] },
      { name: 'lyrel-core', dependencies: [dep('lyrel-protocol')] },
      { name: 'lyrel-protocol', dependencies: [] },
      { name: 'lyrel-document', dependencies: [] },
      { name: 'lyrel-content-diff', dependencies: [dep('tree-sitter')] },
      { name: 'lyrel-insights', dependencies: [dep('lyrel-protocol')] },
      { name: 'lyrel-updater', dependencies: [] },
      { name: 'lyrel-markdown-editor', dependencies: [dep('lyrel-document')] },
      { name: 'lyrel-test-support', dependencies: [] },
      { name: 'lyrel-ui-base', dependencies: [] },
      { name: 'lyrel-terminal', dependencies: [dep('lyrel-ui-base')] },
      { name: 'lyrel-workbench-editor', dependencies: [dep('lyrel-ui-base'), dep('lyrel-document'), dep('lyrel-core')] },
    ],
    normal: new Map([
      ['lyrel-cli', new Set(['lyrel-protocol'])],
      ['lyrel-server', new Set(['lyrel-core', 'lyrel-protocol', 'rusqlite'])],
      ['lyrel-ui-base', new Set(['lyrel-protocol', 'lsp-types'])],
      ['lyrel-terminal', new Set(['lyrel-ui-base', 'lyrel-core', 'russh', 'lsp-types'])],
      ['lyrel-workbench-editor', new Set(['lyrel-ui-base', 'lyrel-document', 'wry'])],
    ]),
    headless: new Set(['lyrel-protocol', 'lyrel-core', 'lyrel-server', 'lyrel-cli', 'lyrel-document', 'lyrel-insights', 'lyrel-test-support']),
  };
}

const check = ({ packages, normal, headless }) => graphViolations(packages, normal, headless);

test('dev-only engines and a standalone updater do not breach the production graph', () => {
  assert.deepEqual(check(graph()), []);
});

test('rejects desktop packages reached indirectly by a Linux headless member', () => {
  const input = graph();
  input.headless.add('lyrel-markdown-editor');
  assert.deepEqual(check(input), ['Linux headless production graph reaches lyrel-markdown-editor']);
});

test('flags a removed engine and an unclassified replacement workspace member', () => {
  const input = graph();
  input.packages.find(pkg => pkg.name === 'lyrel-document').name = 'lyrel-document-next';
  assert.deepEqual(check(input), [
    'Missing workspace member: lyrel-document',
    'Unexpected workspace member: lyrel-document-next',
  ]);
});

test('rejects a transitive daemon dependency in the CLI and transitive insights in server', () => {
  const input = graph();
  input.normal.get('lyrel-cli').add('tokio');
  input.normal.get('lyrel-server').add('lyrel-insights');
  const violations = check(input);
  assert.ok(violations.some(line => /lyrel-cli.*tokio/.test(line)), violations);
  assert.ok(violations.some(line => /lyrel-server.*lyrel-insights/.test(line)), violations);
});

test('rejects a forbidden CLI dependency declared outside the selected targets', () => {
  const input = graph();
  const cli = input.packages.find(pkg => pkg.name === 'lyrel-cli');
  cli.dependencies.push({ ...dep('rusqlite', null, 'db'), target: 'cfg(target_arch = "riscv64")' });
  assert.deepEqual(check(input), ['lyrel-cli declares a direct production dependency on rusqlite']);
});

test('only the workbench owns the document engines in production', () => {
  const input = graph();
  const root = input.packages.find(pkg => pkg.name === 'lyrel');
  root.dependencies.push(dep('lyrel-document', 'dev'), dep('lyrel-markdown-editor', 'dev'));
  assert.deepEqual(check(input), []);
  root.dependencies.push(dep('lyrel-document'), dep('lyrel-markdown-editor'), dep('tree-sitter'));
  assert.deepEqual(check(input), [
    'lyrel declares a direct production dependency on lyrel-document',
    'lyrel declares a direct production dependency on lyrel-markdown-editor',
    'lyrel declares a direct production dependency on tree-sitter',
  ]);
});

test('permits Markdown only through the opt-in native harness', () => {
  const input = graph();
  const root = input.packages.find(pkg => pkg.name === 'lyrel');
  root.features = { default: [], 'native-markdown-tests': ['dep:lyrel-markdown-editor'] };
  root.dependencies.push({ ...dep('lyrel-markdown-editor'), optional: true });
  assert.deepEqual(check(input), []);

  root.features.default.push('native-markdown-tests');
  assert.deepEqual(check(input), ['lyrel declares a direct production dependency on lyrel-markdown-editor']);
});

test('permits Windows browser bindings and keeps wry in the editor', () => {
  const input = graph();
  const root = input.packages.find(pkg => pkg.name === 'lyrel');
  root.dependencies.push({ ...dep('webview2-com'), target: 'cfg(windows)' });
  assert.deepEqual(check(input), []);
  root.dependencies.push({ ...dep('wry'), target: 'cfg(target_os = "macos")' });
  assert.deepEqual(check(input), ['lyrel declares a direct production dependency on wry']);
});

test('keeps the server one edge deep and detects renamed direct GUI engine dependencies', () => {
  const input = graph();
  input.packages.find(pkg => pkg.name === 'lyrel-server').dependencies.push(dep('log'));
  const root = input.packages.find(pkg => pkg.name === 'lyrel');
  root.dependencies.push(dep('webview2-com', null, 'web_surface'));
  root.features.updater = [];
  const violations = check(input);
  assert.ok(violations.some(line => /lyrel-server.*log/.test(line)), violations);
  assert.ok(violations.some(line => /lyrel.*webview2-com/.test(line)), violations);
  assert.ok(violations.some(line => /lyrel.*updater.*feature|lyrel.*feature.*updater/.test(line)), violations);
});

test('rejects a direct editor-to-terminal dev edge and a transitive terminal-to-editor production edge', () => {
  const input = graph();
  input.packages.find(pkg => pkg.name === 'lyrel-workbench-editor').dependencies.push(dep('lyrel-terminal', 'dev'));
  input.normal.get('lyrel-terminal').add('lyrel-workbench-editor');
  const violations = check(input);
  assert.ok(violations.some(line => /lyrel-workbench-editor.*lyrel-terminal/.test(line)), violations);
  assert.ok(violations.some(line => /lyrel-terminal.*lyrel-workbench-editor/.test(line)), violations);
});

test('keeps installer-only archive machinery out of the workbench editor graph', () => {
  const input = graph();
  input.normal.get('lyrel-workbench-editor').add('async_zip');
  const violations = check(input);
  assert.ok(violations.some(line => /lyrel-workbench-editor reaches async_zip/.test(line)), violations);
});

test('rejects the editor re-enabling the remote-install feature on lyrel-core', () => {
  const input = graph();
  const editor = input.packages.find(pkg => pkg.name === 'lyrel-workbench-editor');
  editor.dependencies.find(dependency => dependency.name === 'lyrel-core').features = ['remote-install'];
  assert.deepEqual(check(input), [
    'lyrel-workbench-editor must not enable the remote-install feature on lyrel-core',
  ]);
  // Without the feature the declaration is fine again.
  delete editor.dependencies.find(dependency => dependency.name === 'lyrel-core').features;
  assert.deepEqual(check(input), []);
});

test('terminal may inherit GPUI LSP values but cannot declare its own LSP dependency', () => {
  const input = graph();
  input.packages.find(pkg => pkg.name === 'lyrel-terminal').dependencies.push(dep('lsp-types'));
  assert.deepEqual(check(input), ['lyrel-terminal declares a direct production dependency on lsp-types']);
});

test('keeps shared fixtures out of production and build dependency declarations', () => {
  const input = graph();
  const cli = input.packages.find(pkg => pkg.name === 'lyrel-cli');
  const root = input.packages.find(pkg => pkg.name === 'lyrel');
  cli.dependencies.push(dep('lyrel-test-support', 'dev'));
  assert.deepEqual(check(input), []);
  root.dependencies.push(dep('lyrel-test-support', null, 'fixtures'));
  cli.dependencies.push(dep('lyrel-test-support', 'build'));
  const violations = check(input);
  assert.deepEqual(violations.sort(), [
    'lyrel declares a non-dev dependency on lyrel-test-support',
    'lyrel-cli declares a non-dev dependency on lyrel-test-support',
  ].sort());
});
