#!/usr/bin/env node
import { mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const [command, directory, size, ...extra] = process.argv.slice(2);

if (command === 'hold' && directory === undefined) {
  // The documented agent_commands alias enables the real composer in this
  // pane. This process keeps the pane quiet and needs no agent or provider.
  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    throw new Error('Run hold in the fixture’s terminal pane');
  }
  console.log(`LYREL_SEARCH_READY ${process.cwd()}`);
  process.stdin.on('data', () => console.error('LYREL_SEARCH_UNEXPECTED_INPUT'));
} else if (command === 'prepare' && directory && ['5000', '50000'].includes(size) && !extra.length) {
  const root = resolve(directory);
  const project = join(root, 'project');
  const config = join(root, 'config');
  mkdirSync(root);
  mkdirSync(project);
  mkdirSync(config);

  // Keep the project flat: the production walk counts directories toward
  // its 50,000-entry cap before filtering them out of the index.
  const probes = [
    'fable.txt', 'fritz.txt', 'flare-a.txt', 'flare-b.txt',
    'frost-a.txt', 'frost-b.txt', 'f文-a.txt', 'f文-b.txt',
  ];
  const count = Number(size);
  const writeProbe = name => writeFileSync(join(project, name), `LYREL_SEARCH_PATH=${name}\n`);
  for (const name of probes) writeProbe(name);
  for (let i = 0; i < count - probes.length; i++) {
    writeProbe(`file_${String(i).padStart(5, '0')}.txt`);
  }
  writeFileSync(join(config, 'config.json'), JSON.stringify({
    check_for_updates: false,
    auto_download_updates: false,
    install_cli_on_path: false,
    show_tray_icon: false,
    agent_commands: { 'workspace-search': 'pi' },
  }, null, 2) + '\n');
  console.log(JSON.stringify({ project, config, paths: count }));
} else {
  throw new Error('Usage: node scripts/bench/workspace-search.mjs prepare <new-root> <5000|50000> or hold');
}
