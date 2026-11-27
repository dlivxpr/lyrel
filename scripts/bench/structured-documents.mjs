import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const usage = `node scripts/bench/structured-documents.mjs prepare <new-root>
node scripts/bench/structured-documents.mjs verify <root> <original|edited> <tree|table|all>`;
const treeEntries = 3_500;
const tableEntries = 50_000;
const names = { tree: '443-tree.jsonc', table: '443-table.csv' };
const markers = {
  tree: { original: 'TREE-1111', edited: 'TREE-2229' },
  table: { original: 'CELL-1111', edited: 'CELL-2229' },
};

function treeFixture(marker) {
  // The only change between expected revisions is the second phase occurrence.
  const rows = Array.from({ length: treeEntries }, (_, index) => {
    const id = String(index).padStart(5, '0');
    return `    {"id":"${id}","text":"${'config-sample-'.repeat(12)}${id}","gain":1.234567890123456789e+12},\n`;
  }).join('');
  return Buffer.from(
    '\ufeff{\r\n  // Keep this comment, the mixed line endings and the duplicate key.\n'
    + '  "ready": "TREE-READY",\r\n  "probe": {\n    "target": {\r\n'
    + `      "phase": "first\\u4e2d",\n      "phase": "${marker}"\r\n`
    + '    }\n  },\r\n  "payload": [\n'
    + rows
    + '  ],\r\n}\n',
  );
}

function tableFixture(marker) {
  // Reuse the existing BOM, quoted comma, multiline field and ragged records.
  // Normalize checkout line endings to make the workload platform-independent.
  const seed = readFileSync(new URL('../../src/ui/code_editor/csv_document/fixtures/table.csv', import.meta.url), 'utf8')
    .replaceAll('\r\n', '\n');
  const headerEnd = seed.indexOf('\n') + 1;
  const rows = Array.from({ length: tableEntries }, (_, index) => {
    const id = String(index).padStart(5, '0');
    const eol = index % 2 === 0 ? '\r\n' : '\n';
    return `ROW-${id},"factory,west","中文 item ${id} with ""quoted"" value"${eol}`;
  }).join('');
  return Buffer.from(
    seed.slice(0, headerEnd)
    + `table-ready,probe,${marker}\r\n`
    + seed.slice(headerEnd)
    + (seed.endsWith('\n') ? '' : '\n')
    + rows,
  );
}

function fixture(root, kind, revision) {
  const path = join(root, 'workspace', names[kind]);
  const marker = markers[kind][revision];
  const bytes = kind === 'tree' ? treeFixture(marker) : tableFixture(marker);
  return { path, bytes, marker };
}

function identity({ path, bytes, marker }) {
  return {
    path,
    marker,
    bytes: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex'),
  };
}

function main(args) {
  const [command, directory, revision, kind] = args;
  if (command === '--help' && args.length === 1) {
    console.log(usage);
    return;
  }
  if (command === 'prepare' && args.length === 2 && directory) {
    const root = resolve(directory);
    const workspace = join(root, 'workspace');
    const config = join(root, 'config');
    mkdirSync(root);
    mkdirSync(workspace);
    mkdirSync(config);
    writeFileSync(join(config, 'config.json'), '{"gui_language":"en","document_layout":"dock"}\n');
    const originals = Object.keys(names).map(format => fixture(root, format, 'original'));
    for (const { path, bytes } of originals) writeFileSync(path, bytes);
    console.log(JSON.stringify({ workspace, config, files: originals.map(identity) }, null, 2));
    return;
  }
  if (command === 'verify' && args.length === 4 && directory
      && ['original', 'edited'].includes(revision) && ['tree', 'table', 'all'].includes(kind)) {
    const formats = kind === 'all' ? Object.keys(names) : [kind];
    const files = formats.map(format => {
      const expected = fixture(resolve(directory), format, revision);
      const actual = readFileSync(expected.path);
      if (!actual.equals(expected.bytes)) {
        const want = identity(expected);
        const got = identity({ ...expected, bytes: actual });
        throw new Error(`${expected.path} differs from ${revision}: expected ${want.bytes} bytes (${want.sha256}), got ${got.bytes} bytes (${got.sha256})`);
      }
      return identity(expected);
    });
    console.log(JSON.stringify({ revision, files }, null, 2));
    return;
  }
  throw new Error(usage);
}

try {
  main(process.argv.slice(2));
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
