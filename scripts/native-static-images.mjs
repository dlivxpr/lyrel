#!/usr/bin/env node
// Test-only adapter around pinned native components. No Kitty encoder, PTY text
// parser, clipboard mutation, CLI prompt, provider or replacement renderer here.
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { encodePng, fixturePicture } from './graphics-fixture-utils.mjs';

const args = process.argv.slice(2);
const option = (name, fallback) => { const i = args.indexOf(name); return i < 0 ? fallback : args[i + 1]; };
const agent = option('--agent');
const policy = option('--policy', 'auto');
const source = realpathSync(option('--source'));
const pins = { kimi: 'e2770e37bd198e5208be0e0b60b48c77789cf94b', hermes: '3b421578588243f920886e6fe79a6faa87ea026e' };
assert(agent in pins && ['auto', 'off'].includes(policy));
assert(process.stdin.isTTY && process.stdout.isTTY, 'Requires owned real PTY');
assert.equal(execFileSync('git', ['-C', source, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(), pins[agent], 'Use an isolated pinned upstream checkout; never upgrade a global agent');
execFileSync('git', ['-C', source, 'diff', '--exit-code', 'HEAD', '--', '.'], { stdio: 'pipe' });

function picture(index) {
  const { width, height, scan } = fixturePicture(index);
  return encodePng(width, height, scan);
}

if (!args.includes('--worker')) {
  const root = mkdtempSync(path.join(path.resolve(option('--temp-parent', tmpdir())), 'lyrel-native-static-'));
  let child;
  try {
    for (const dir of ['home', 'config', 'cache', 'data', 'state', 'runtime', 'tmp', 'hermes']) mkdirSync(path.join(root, dir));
    for (let i = 0; i < 2; i++) writeFileSync(path.join(root, `image-${i}.png`), picture(i));
    writeFileSync(path.join(root, 'hermes', 'config.yaml'), `display:\n  inline_images: ${policy}\n`);
    const keep = ['PATH', 'Path', 'PATHEXT', 'SystemRoot', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'TERM', 'COLORTERM',
      'TERM_PROGRAM', 'TERM_PROGRAM_VERSION', 'LC_TERMINAL', 'LC_TERMINAL_VERSION', 'LYREL', 'KITTY_WINDOW_ID', 'LANG', 'LC_ALL'];
    const env = Object.fromEntries(keep.filter(k => process.env[k] !== undefined).map(k => [k, process.env[k]]));
    Object.assign(env, { HOME: path.join(root, 'home'), USERPROFILE: path.join(root, 'home'),
      APPDATA: path.join(root, 'config'), LOCALAPPDATA: path.join(root, 'data'),
      XDG_CONFIG_HOME: path.join(root, 'config'), XDG_CACHE_HOME: path.join(root, 'cache'), XDG_DATA_HOME: path.join(root, 'data'),
      XDG_STATE_HOME: path.join(root, 'state'), XDG_RUNTIME_DIR: path.join(root, 'runtime'),
      TMP: path.join(root, 'tmp'), TEMP: path.join(root, 'tmp'), TMPDIR: path.join(root, 'tmp'),
      HERMES_HOME: path.join(root, 'hermes'), HERMES_RUNTIME_DIR: path.join(root, 'runtime'), PYTHONUTF8: '1',
      PYTHONDONTWRITEBYTECODE: '1', FORCE_COLOR: '3' });
    assert.equal(env.TERM_PROGRAM, 'kitty');
    assert.equal(env.TERM_PROGRAM_VERSION, '');
    assert.equal(env.LC_TERMINAL, 'lyrel');
    assert(env.LC_TERMINAL_VERSION && env.LYREL);
    if (agent === 'hermes') assert.equal(env.KITTY_WINDOW_ID, 'lyrel', 'Hermes native auto detector requires its scoped marker');
    if (agent === 'kimi' && policy === 'off') env.PI_IMAGE_PROTOCOL = 'none';
    const loader = path.join(source, agent === 'kimi' ? 'node_modules' : 'ui-tui/node_modules', 'tsx/dist/cli.mjs');
    // Resolve compiler settings from the pinned source, not the isolated cwd.
    // Hermes' native TSX uses the automatic JSX runtime (no React default import).
    // Kimi also needs its own ?raw loader for transitive system.md imports.
    const compiler = ['--tsconfig', path.join(source, agent === 'kimi' ? 'tsconfig.json' : 'ui-tui/tsconfig.json'),
      ...(agent === 'kimi' ? ['--import', pathToFileURL(path.join(source, 'build/register-raw-text-loader.mjs')).href] : [])];
    const worker = [loader, ...compiler, fileURLToPath(import.meta.url), ...args, '--worker', '--root', root,
      '--python', option('--python', process.env.LYREL_HERMES_GRAPHICS_PYTHON ?? 'python')];
    child = spawn(process.execPath, worker, { cwd: root, env, stdio: 'inherit' });
    const timer = setTimeout(() => child.kill(), 180_000);
    const stop = () => child.kill();
    process.once('SIGTERM', stop); process.once('SIGINT', stop);
    const result = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', (code, signal) => resolve({ code, signal })); });
    clearTimeout(timer); process.removeListener('SIGTERM', stop); process.removeListener('SIGINT', stop);
    assert.equal(result.code, 0, JSON.stringify(result));
    assert.equal(readFileSync(path.join(root, 'hermes/config.yaml'), 'utf8'), `display:\n  inline_images: ${policy}\n`);
    console.log('NATIVE_STATIC_AUDITED: native components only; no prompt/provider; owned config unchanged');
  } finally {
    child?.kill();
    rmSync(root, { recursive: true, force: true });
  }
} else {
  const root = option('--root');
  const imported = rel => import(pathToFileURL(path.join(source, rel)).href);
  const before = [0, 1].map(i => createHash('sha256').update(readFileSync(path.join(root, `image-${i}.png`))).digest('hex'));
  let advance;
  const next = () => new Promise(resolve => { advance = resolve; });
  const onInput = data => { if (String(data).includes('n') && advance) { const resolve = advance; advance = undefined; resolve(); } };
  process.stdin.on('data', onInput);
  let show, empty, stop;
  if (agent === 'kimi') {
    const pkg = JSON.parse(readFileSync(path.join(source, 'apps/kimi-code/package.json'), 'utf8'));
    assert.equal(pkg.name, '@moonshot-ai/kimi-code');
    assert.equal(pkg.version, '2.5.1'); // source commit, not the installed 2.2.0 or an inferred 2.5.2 release
    const tui = await imported('packages/pi-tui/src/index.ts');
    const { UserMessageComponent } = await imported('apps/kimi-code/src/tui/components/messages/user-message.ts');
    const { ImageAttachmentStore } = await imported('apps/kimi-code/src/tui/utils/image-attachment-store.ts');
    assert.equal(tui.getCapabilities().images, policy === 'off' ? null : 'kitty');
    const store = new ImageAttachmentStore();
    const ui = new tui.TuiAltScreen(new tui.ProcessTerminal(), false, undefined, { mouse: false });
    ui.start();
    show = (phase, index) => {
      ui.clear();
      const image = store.addImage(readFileSync(path.join(root, `image-${index}.png`)), 'image/png', 48, 24);
      ui.addChild(new UserMessageComponent(`NATIVE_STATIC_${phase}`, [image]));
      ui.addChild(new tui.Text('NATIVE_STATIC_AFTER\nLiteral <svg/> ![not image](https://example.invalid/x.png)', 0, 0));
      ui.requestRender();
    };
    empty = () => { store.clear(); ui.clear(); ui.addChild(new tui.Text('NATIVE_STATIC_EMPTY', 0, 0)); ui.requestRender(); };
    stop = () => ui.stop();
  } else {
    const req = createRequire(path.join(source, 'ui-tui/package.json'));
    const React = req('react');
    // Enter native Ink through ESM, so its import-only dependencies (including
    // ansi-tokenize) use their supported exports on both Node 22.19 and 26.
    const { Box, Text, render } = await import(pathToFileURL(req.resolve('@hermes/ink')).href);
    const { InlineImage } = await imported('ui-tui/src/components/inlineImage.tsx');
    const { GatewayProvider } = await imported('ui-tui/src/app/gatewayContext.tsx');
    const { detectTerminalGraphics } = await imported('ui-tui/src/lib/terminalGraphics.ts');
    assert.equal(detectTerminalGraphics(), 'kitty');
    const helper = path.join(path.dirname(fileURLToPath(import.meta.url)), 'hermes-native-image-rpc.py');
    let calls = 0;
    let rendered;
    const gw = { request: async (method, params) => {
      assert.equal(method, 'image.render', 'Only native public image.render is permitted');
      calls++;
      const response = JSON.parse(execFileSync(option('--python'), [helper, '--source', source], {
        input: JSON.stringify({ id: calls, method, params }), encoding: 'utf8', timeout: 30_000,
      }));
      assert(!response.error, JSON.stringify(response));
      assert.equal(response.result.mode, policy === 'off' ? 'off' : 'kitty');
      rendered?.(true);
      return response.result;
    } };
    const h = React.createElement;
    // Pinned Ink's public render() is async; rerender belongs to its resolved Instance.
    const ui = await render(h(Text, null, 'NATIVE_STATIC_STARTING'), { exitOnCtrlC: false, patchConsole: false });
    // Native App.componentDidCatch rejects this public lifecycle promise. Without
    // observing it, a component error can leave only the last committed frame.
    let phase = 'startup';
    void ui.waitUntilExit().catch(error => {
      process.nextTick(() => { throw new Error(`Hermes native render failed during ${phase}`, { cause: error }); });
    });
    process.stdin.setRawMode(true); process.stdin.resume();
    function Frame({ phase, index }) {
      const [ready, setReady] = React.useState(false);
      rendered = setReady;
      return h(GatewayProvider, { value: { gw } }, h(Box, { flexDirection: 'column' },
        h(Text, null, ready ? `NATIVE_STATIC_${phase}` : `NATIVE_STATIC_LOADING_${phase}`),
        h(InlineImage, { cols: 64, path: path.join(root, `image-${index}.png`), revision: phase, key: phase }),
        h(Text, null, 'NATIVE_STATIC_AFTER\nLiteral <svg/> ![not image](https://example.invalid/x.png)')));
    }
    show = (nextPhase, index) => { phase = nextPhase; ui.rerender(h(Frame, { phase, index, key: phase })); };
    empty = () => { phase = 'unmount'; ui.rerender(h(Text, null, 'NATIVE_STATIC_EMPTY')); };
    stop = () => { assert(calls >= 3); ui.unmount(); process.stdin.setRawMode(false); };
  }
  try {
    for (const [phase, index] of [['ATTACHMENT', 0], ['REMOUNT', 0], ['REPLACEMENT', 1]]) {
      show(phase, index); await next(); empty(); await next();
    }
  } finally {
    stop(); process.stdin.off('data', onInput); process.stdin.pause();
  }
  for (let i = 0; i < 2; i++) assert.equal(createHash('sha256').update(readFileSync(path.join(root, `image-${i}.png`))).digest('hex'), before[i]);
}
