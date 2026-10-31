#!/usr/bin/env node
// Test-only public OpenCode session import/export driver. Never a production
// history reader or graphics sender: only the released TUI emits image bytes.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { encodePng } from './graphics-fixture-utils.mjs';

const VERSION = '2.2.0';
const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  if (index < 0) return undefined;
  assert(args[index + 1] && !args[index + 1].startsWith('--'), `${name} requires a value`);
  return args[index + 1];
};
const prepareRoot = option('--prepare');
const resumeRoot = option('--resume');
const previews = option('--previews');
const binary = option('--opencode') ?? 'opencode';
const allowed = new Set(['--prepare', '--resume', '--previews', '--opencode']);
for (let i = 0; i < args.length; i += 2) assert(allowed.has(args[i]), `Unknown option: ${args[i]}`);
assert(!(prepareRoot && resumeRoot), '--prepare and --resume are mutually exclusive');
assert(resumeRoot || previews === 'on' || previews === 'off', 'Explicitly select --previews on or --previews off; no user policy is overwritten');
assert(!resumeRoot || previews === undefined, '--resume uses the saved fixture display policy');

function environment(root) {
  // Whitelist runtime/terminal facts, never copy credentials, provider config,
  // private histories, global plugins, service URLs or inherited CLI overlays.
  const keep = ['PATH', 'Path', 'SystemRoot', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT',
    'LANG', 'LC_ALL', 'TERM', 'TERM_PROGRAM', 'TERM_PROGRAM_VERSION', 'COLORTERM',
    'LYREL', 'LYREL_PANE', 'LYREL_CONFIG_DIR', 'LYREL_DATA_DIR', 'LYREL_CONTROL_SOCK'];
  const env = Object.fromEntries(keep.filter((key) => process.env[key] !== undefined).map((key) => [key, process.env[key]]));
  Object.assign(env, {
    HOME: path.join(root, 'home'), USERPROFILE: path.join(root, 'home'),
    APPDATA: path.join(root, 'home', 'AppData', 'Roaming'), LOCALAPPDATA: path.join(root, 'home', 'AppData', 'Local'),
    XDG_CONFIG_HOME: path.join(root, 'config'), XDG_DATA_HOME: path.join(root, 'data'),
    XDG_STATE_HOME: path.join(root, 'state'), XDG_CACHE_HOME: path.join(root, 'cache'),
    XDG_RUNTIME_DIR: path.join(root, 'runtime'), TMP: path.join(root, 'tmp'), TEMP: path.join(root, 'tmp'), TMPDIR: path.join(root, 'tmp'),
  });
  return env;
}

function run(root, command, interactive = false) {
  const result = spawnSync(binary, command, {
    cwd: path.join(root, 'work'), env: environment(root), shell: false,
    stdio: interactive ? 'inherit' : ['ignore', 'pipe', 'pipe'], encoding: 'utf8',
    ...(interactive ? {} : { timeout: 120_000, maxBuffer: 16 * 1024 * 1024 }),
  });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `OpenCode ${command.join(' ')} failed (${result.signal ?? result.status}): ${result.stderr ?? ''}`);
  return result.stdout ?? '';
}

function picture(rgb, ordinal) {
  const width = 96, height = 48;
  const rows = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    // Black border, white ordinal bars, saturated field: visibly distinct and
    // stable under the native card's cover scaling. Never write a PNG cache.
    const border = x < 2 || y < 2 || x >= width - 2 || y >= height - 2;
    const bar = y >= 18 && y < 30 && x >= 12 && x < 12 + ordinal * 10 && x % 10 < 6;
    const color = border ? [0, 0, 0] : bar ? [255, 255, 255] : rgb;
    rows.set([...color, 255], y * (width * 4 + 1) + 1 + x * 4);
  }
  return encodePng(width, height, rows).toString('base64');
}

function transcript(directory) {
  const session = `ses_lyrel_${randomUUID().replaceAll('-', '')}`;
  const messages = [];
  const time = 1_793_404_800_000;
  const add = (message) => messages.push({ id: `msg_lyrel_${String(messages.length).padStart(6, '0')}`, time: { created: time + messages.length }, ...message });
  const model = { providerID: 'anthropic', id: 'claude-sonnet-4-5' };
  const palette = {
    OLD: [[220, 0, 0], [0, 180, 0], [0, 0, 220], [220, 0, 220], [0, 180, 180], [180, 180, 0], [220, 100, 0], [120, 0, 180]],
    TAIL: [[240, 70, 70], [70, 210, 70], [70, 70, 240], [240, 70, 240], [70, 210, 210], [210, 210, 70], [240, 140, 70], [170, 70, 210]],
  };
  const cards = (label) => {
    const pictures = palette[label].map((rgb, i) => picture(rgb, i % 4 + 1));
    add({ type: 'user', text: `${label}_USER: red/green/blue; fourth magenta is not inline. Literal <svg>not a picture</svg>, ![literal](https://example.invalid/never.png), C:\\not-a-picture.png`,
      files: pictures.slice(0, 4).map((data, i) => ({ data, mime: 'image/png', name: `${label}-user-${i + 1}.png`, source: { type: 'inline' } })) });
    add({ type: 'assistant', agent: 'build', model, finish: 'stop',
      time: { created: time + messages.length, completed: time + messages.length + 1 },
      content: [{ type: 'text', text: `${label}_TOOL: cyan/yellow/orange; fourth violet is not inline.` },
        { type: 'tool', id: `call_${label}`, name: 'read', executed: true,
          time: { created: time + messages.length, completed: time + messages.length + 1 },
          state: { status: 'completed', input: { filePath: `${label}_TOOL` }, content: [
            { type: 'text', text: `${label}_TOOL saved synthetic result, not a live tool execution` },
            ...pictures.slice(4).map((data, i) => ({ type: 'file', mime: 'image/png', name: `${label}-tool-${i + 1}.png`, uri: `data:image/png;base64,${data}` })),
            { type: 'file', mime: 'image/png', name: 'unsupported-remote-reference.png', uri: 'https://example.invalid/never.png' },
          ] } }] });
    add({ type: 'idle', outcome: 'succeeded' });
  };
  cards('OLD');
  for (let i = 0; i < 180; i++) {
    add({ type: 'user', text: `PAGE_${String(i).padStart(3, '0')} isolated history navigation filler` });
    add({ type: 'assistant', agent: 'build', model, finish: 'stop',
      time: { created: time + messages.length, completed: time + messages.length + 1 },
      content: [{ type: 'text', text: `PAGE_${String(i).padStart(3, '0')} saved response; no inference was requested.` }] });
    add({ type: 'idle', outcome: 'succeeded' });
  }
  cards('TAIL');
  return { info: { id: session, projectID: 'global', agent: 'build', model,
    title: 'lyrel isolated OpenCode native image replay', location: { directory }, cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    time: { created: time, updated: time + messages.length, idle: time + messages.length }, outcome: 'succeeded' }, messages };
}

function exported(root, session) {
  const response = JSON.parse(run(root, ['session', 'export', session, '--standalone']));
  const data = response.info ? response : response.data;
  assert(data?.info && Array.isArray(data.messages), 'Public session export returned an unexpected schema');
  return data;
}
function verify(root, manifest) {
  const after = exported(root, manifest.session);
  assert.deepEqual(after.messages, manifest.savedMessages, 'Replay changed saved messages (prompt, inference, or source mutation)');
  assert.equal(after.info.cost, 0, 'Replay incurred model cost');
  assert.deepEqual(after.info.tokens, { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } }, 'Replay consumed model tokens');
  const cli = JSON.parse(readFileSync(path.join(root, 'config', 'opencode', 'cli.json'), 'utf8'));
  assert.equal(cli.session.image_preview, manifest.previews, 'Native replay changed the explicit display policy');
  console.log('OPENCODE_REPLAY_VERIFIED: unchanged public export, zero model usage');
}
function prepare(root) {
  mkdirSync(root, { recursive: true });
  assert.equal(readdirSync(root).length, 0, '--prepare requires an empty, owned fixture directory');
  root = realpathSync(root);
  for (const dir of ['home', 'config/opencode', 'data', 'state', 'cache', 'runtime', 'tmp', 'work']) mkdirSync(path.join(root, dir), { recursive: true, mode: 0o700 });
  const version = run(root, ['--version']).trim();
  assert.match(version, /^opencode v2\.2\.0$/, `Expected released OpenCode ${VERSION}; inspected fixtures must be revalidated for other versions`);
  const cli = { $schema: 'https://opencode.ai/v2/cli.json', highlights: false, animations: false,
    attention: { sound: false, notifications: false }, prompt: { editor: false, image_preview: false },
    tabs: { mode: 'on' }, session: { image_preview: previews === 'on', grouping: 'none', sidebar: 'hide', verbosity: 'high' },
    keybinds: { 'session.tab.close': 'f6', 'session.tab.reopen': 'f7' } };
  writeFileSync(path.join(root, 'config', 'opencode', 'cli.json'), JSON.stringify(cli, null, 2));
  const source = transcript(path.join(root, 'work'));
  const file = path.join(root, 'saved-session.json');
  writeFileSync(file, JSON.stringify(source));
  run(root, ['session', 'import', file, '--directory', path.join(root, 'work'), '--standalone']);
  const saved = exported(root, source.info.id);
  assert.deepEqual(saved.messages, source.messages, 'Public import did not preserve the synthetic saved image conversation');
  const manifest = { owner: 'lyrel-opencode-native-replay', version: VERSION, session: source.info.id,
    previews: previews === 'on', messageCount: saved.messages.length, savedMessages: saved.messages };
  writeFileSync(path.join(root, 'manifest.json'), JSON.stringify(manifest));
  return { root, manifest };
}
function resume(root) {
  root = realpathSync(root);
  const manifest = JSON.parse(readFileSync(path.join(root, 'manifest.json'), 'utf8'));
  assert.equal(manifest.owner, 'lyrel-opencode-native-replay', 'Not an owned synthetic fixture');
  assert.equal(manifest.version, VERSION);
  assert.match(run(root, ['--version']).trim(), /^opencode v2\.2\.0$/);
  // No --prompt, run, SDK prompt API, source patch, protocol injection or TERM spoof.
  run(root, ['--standalone', '-s', manifest.session], true);
  verify(root, manifest);
}

let owned;
try {
  if (prepareRoot) {
    const prepared = prepare(path.resolve(prepareRoot));
    console.log(JSON.stringify({ root: prepared.root, version: VERSION, session: prepared.manifest.session,
      previews: prepared.manifest.previews, messageCount: prepared.manifest.messageCount }));
  } else if (resumeRoot) {
    resume(path.resolve(resumeRoot));
  } else {
    owned = mkdtempSync(path.join(tmpdir(), 'lyrel-opencode-native-'));
    const prepared = prepare(owned);
    console.log(`OpenCode ${VERSION}; session.image_preview=${prepared.manifest.previews}; ${prepared.manifest.messageCount} saved messages.`);
    console.log('Home/Ctrl+G: OLD history; End: TAIL; F6: close/delete mounted pictures; F7: reopen/remount; Ctrl+C: exit. Do not submit a prompt.');
    console.log('Each user/tool card: first three colored/barred PNGs only, +1 more; no magenta/violet inline.');
    resume(owned);
  }
} finally {
  // Only remove the directory created by this invocation. Explicit --prepare /
  // --resume stores remain owned by the caller (e.g. Rust TempDir), never users.
  if (owned && existsSync(owned)) rmSync(owned, { recursive: true, force: true });
}
