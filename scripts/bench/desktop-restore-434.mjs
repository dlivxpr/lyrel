import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { appendFile, lstat, mkdir, mkdtemp, open, readFile, realpath, stat, writeFile } from 'node:fs/promises';
import { arch, cpus, release, tmpdir, totalmem } from 'node:os';
import { basename, dirname, join, resolve, sep } from 'node:path';
import { createInterface } from 'node:readline/promises';
import { setTimeout as delay } from 'node:timers/promises';
import { parseArgs, promisify } from 'node:util';

const exec = promisify(execFile);
const windows = process.platform === 'win32';
const pollMs = 20;
const now = () => Number(process.hrtime.bigint()) / 1e6;
const usage = `node scripts/bench/desktop-restore-434.mjs --app RELEASE_APP --cli RELEASE_CLI
  --revision COMMIT [--bash GIT_BASH_EXE] [--cycles 1] [--steady-ms 2000]
Windows 需要 --bash 指向 Git for Windows 的原生 bash.exe。macOS 使用 /bin/bash。`;

const { values: options } = parseArgs({ options: {
  app: { type: 'string' }, cli: { type: 'string' }, bash: { type: 'string' },
  revision: { type: 'string' },
  cycles: { type: 'string', default: '1' }, 'steady-ms': { type: 'string', default: '2000' },
  help: { type: 'boolean' },
} });
if (options.help) {
  console.log(usage);
  process.exit(0);
}
assert(windows || process.platform === 'darwin', '需要 Windows 或 macOS 原生桌面');
assert(options.app && options.cli && options.revision, usage);
assert(windows ? options.bash : !options.bash, usage);
const cycles = Number(options.cycles);
const steadyMs = Number(options['steady-ms']);
assert(Number.isSafeInteger(cycles) && cycles >= 1, '--cycles 必须为正整数');
assert(Number.isSafeInteger(steadyMs) && steadyMs >= 0, '--steady-ms 必须为非负整数');

const [app, cli, bash] = await Promise.all(
  [options.app, options.cli, options.bash ?? '/bin/bash'].map(path => realpath(path)),
);
for (const [file, name] of [[app, 'lyrel-app'], [cli, 'lyrel'], [bash, 'bash']]) {
  assert.equal(basename(file).toLowerCase(), windows ? `${name}.exe` : name);
  assert((await stat(file)).isFile(), `${file} 不是文件`);
}
for (const binary of [app, cli]) {
  const folders = dirname(binary).split(sep).map(part => part.toLowerCase());
  assert(folders.at(-1) === 'release' && folders.includes('target'), `使用 Cargo target/.../release 中的原始产物：${binary}`);
  if (windows) {
    // Startup reconciles portable backups even with update checks disabled.
    for (const name of ['.lyrel-portable', '.lyrel-inno-install']) {
      const marker = join(dirname(binary), name);
      try {
        await lstat(marker);
        assert.fail(`发现安装标记 ${marker}，需要独立的 Cargo 构建目录`);
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
      }
    }
  } else {
    // A bundle's updater can sweep its containing directory at startup.
    assert(!folders.some(part => part.endsWith('.app')), `使用 .app 之外的 Cargo 原始产物：${binary}`);
  }
}

// Inherit only OS/locale settings. Agent-specific selectors, IPC overrides,
// login rc overrides and loader variables must not reach the fixture.
const systemEnv = Object.fromEntries(Object.entries(process.env).flatMap(([key, value]) => {
  const name = windows ? key.toUpperCase() : key;
  return /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|COMSPEC|PROGRAMDATA|LANG|LC_[A-Z_]+|TERM)$/.test(name)
    ? [[name, value]] : [];
}));
let loginShell;
if (!windows) {
  systemEnv.PATH = '/usr/bin:/bin:/usr/sbin:/sbin';
  // GUI startup probes the account's passwd shell independently of the pane
  // shell and $SHELL. Only the system shells' startup paths are covered here.
  const { stdout } = await exec('/usr/bin/id', ['-P'], { env: systemEnv, timeout: 5_000 });
  const fields = stdout.trimEnd().split(':');
  loginShell = fields[9];
  assert(fields.length === 10 && ['/bin/zsh', '/bin/bash'].includes(loginShell),
    'macOS 登录 Shell 必须为系统 /bin/zsh 或 /bin/bash，且使用系统自带的全局启动脚本');
}
const root = await mkdtemp(join(await realpath(windows ? tmpdir() : '/tmp'), 'lyrel434-'));
const configDir = join(root, 'config');
const fixtureHome = join(root, 'home');
const temp = join(root, 'tmp');
const isolatedEnv = {
  HOME: fixtureHome, USERPROFILE: fixtureHome,
  APPDATA: join(fixtureHome, 'roaming'), LOCALAPPDATA: join(fixtureHome, 'local'),
  XDG_CONFIG_HOME: join(fixtureHome, '.config'), XDG_DATA_HOME: join(fixtureHome, '.local', 'share'),
  XDG_STATE_HOME: join(fixtureHome, '.local', 'state'), XDG_CACHE_HOME: join(fixtureHome, '.cache'),
  XDG_RUNTIME_DIR: join(root, 'runtime'),
  TMP: temp, TEMP: temp, TMPDIR: temp,
  ZDOTDIR: fixtureHome,
};
const env = { ...systemEnv, ...isolatedEnv };
const journal = join(root, 'events.jsonl');
const ackFile = join(root, 'shell.tsv');
const fixture = ['left', 'right', 'anchor'];
const paths = Object.fromEntries(fixture.map(side => [side, join(root, side)]));
// GUI and its daemon use the explicit flag. The standalone CLI uses its
// private environment; never declare this override in the control terminal.
const cliEnv = { ...env, LYREL_CONFIG_DIR: configDir };
await Promise.all([...new Set(Object.values(isolatedEnv))]
  .map(path => mkdir(path, { recursive: true, mode: 0o700 })));
await Promise.all([configDir, ...Object.values(paths)].map(path => mkdir(path)));
await writeFile(ackFile, '');
console.log(`隔离目录 ${root}`);

const config = {
  gui_language: 'en', restore_session: true, install_cli_on_path: false,
  check_for_updates: false, show_tray_icon: false,
  agent_launch: {},
  shell: {
    program: bash,
    args: ['--noprofile', '--rcfile', join(configDir, 'fixture.bash').replaceAll('\\', '/'), '-pi'],
  },
  env: {
    // On Windows the daemon refreshes pane environment variables from HKLM /
    // HKCU. Config overrides win over those values as well as inherited ones.
    ...isolatedEnv,
    INPUTRC: join(configDir, 'inputrc').replaceAll('\\', '/'),
    LYREL_RESTORE434_ACK: ackFile.replaceAll('\\', '/'),
  },
  keybindings: { HibernateTab: 'f8' },
};
await Promise.all([
  writeFile(join(configDir, 'fixture.bash'), await readFile(new URL('./desktop-restore-434.bash', import.meta.url))),
  writeFile(join(configDir, 'inputrc'), ''),
  writeConfig(),
]);

const input = createInterface({ input: process.stdin, output: process.stdout });
const abort = new AbortController();
let workspace;
let gui;
let launched = false;
let stage = 'setup';
const children = new Set();
let succeeded = false;
function interrupt() { abort.abort(new Error('场景已取消')); }
input.on('SIGINT', interrupt);
process.on('SIGINT', interrupt);
process.on('SIGTERM', interrupt);

function writeConfig() {
  return writeFile(join(configDir, 'config.json'), `${JSON.stringify(config, null, 2)}\n`);
}

async function event(kind, values = {}) {
  await appendFile(journal, `${JSON.stringify({ stage, kind, hrtimeMs: now(), ...values })}\n`);
}

async function digest(path) {
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  return hash.digest('hex');
}

async function lyrel(...args) {
  const { stdout } = await exec(cli, ['--json', ...args], {
    env: cliEnv, cwd: root, timeout: 5_000, windowsHide: true, maxBuffer: 1024 * 1024,
  });
  return JSON.parse(stdout);
}

async function until(label, probe, ms = 15_000, interval = 100) {
  const expires = now() + ms;
  do {
    abort.signal.throwIfAborted();
    const result = await probe();
    if (result) return result;
    await delay(interval, undefined, { signal: abort.signal });
  } while (now() < expires);
  throw new Error(`等待 ${label} 超时`);
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === 'ESRCH') return false;
    if (error.code === 'EPERM') return true;
    throw error;
  }
}

async function launch(initialCwd) {
  const log = await open(join(root, 'gui.log'), 'a');
  const start = now();
  const args = ['--config-dir', configDir];
  if (initialCwd) args.push('--open-path', initialCwd);
  const child = spawn(app, args, { env, cwd: root, stdio: ['ignore', log.fd, log.fd] });
  const exited = new Promise(resolveExit => {
    const done = result => {
      children.delete(instance);
      resolveExit(result);
    };
    child.once('exit', (code, signal) => done({ code, signal }));
    child.once('error', error => done({ error: error.message }));
  });
  const instance = { child, exited, start };
  children.add(instance);
  await log.close();
  assert(child.pid, `无法启动 ${app}`);
  launched = true;
  await event('launch', { pid: child.pid, hrtimeMs: start });
  return instance;
}

async function waitForExit(instance) {
  await until(`GUI ${instance.child.pid} 退出`, () =>
    instance.child.exitCode !== null || instance.child.signalCode !== null);
  assert.deepEqual(await instance.exited, { code: 0, signal: null });
}

function ask(text) { return input.question(`${text}\n> `, { signal: abort.signal }); }
function paneIds(node) { return node.Leaf ? [node.Leaf.pane] : [...paneIds(node.Split.a), ...paneIds(node.Split.b)]; }
function cwdKey(path) {
  const normalized = resolve(path);
  return windows ? normalized.toLowerCase() : normalized;
}

async function receipt(token) {
  const lines = (await readFile(ackFile, 'utf8')).split('\n');
  lines.pop(); // Ignore a write that has not yet completed its line.
  const matches = lines.filter(line => line.startsWith(`${token}\t`));
  if (!matches.length) return;
  assert.equal(matches.length, 1, `命令 ${token} 被执行多次`);
  const [, id, owner, cwd] = matches[0].split('\t');
  assert(Number.isSafeInteger(Number(id)) && Number(id) > 0 && owner && cwd, `无效 shell 回执 ${token}`);
  return { token, pane: Number(id), owner, cwd };
}

async function accepted(token, side, start) {
  const ack = await until(`桌面输入 ${token} 的 Shell 回执`, () => receipt(token), 45_000, pollMs);
  const observed = now();
  assert.equal(cwdKey(ack.cwd), cwdKey(paths[side]), `输入 ${token} 到达错误目录`);
  if (workspace) assert.equal(ack.owner, workspace, `输入 ${token} 到达错误工作区`);
  await event('accepted', { ...ack, side, hrtimeMs: observed, observedMs: start === undefined ? null : observed - start });
  return ack;
}

async function snapshot() {
  const tree = await lyrel('ws', 'tree', workspace);
  assert.equal(tree.id, workspace);
  assert.equal(tree.name, 'Workspace434');
  assert.equal(tree.tabs.length, 2);
  assert.equal(tree.tabs[0].name, 'Restore434');
  assert.equal(tree.tabs[1].name, 'Anchor434');
  const split = tree.tabs[0].root.Split;
  assert(split?.axis === 'horizontal' && split.a.Leaf && split.b.Leaf, 'Restore434 必须为两个并排平铺窗格');
  assert(tree.tabs[1].root.Leaf, 'Anchor434 必须为单窗格');
  const [left, right, anchor] = tree.tabs.flatMap(tab => paneIds(tab.root));
  const listed = (await lyrel('pane', 'ls', workspace)).panes;
  const registered = (await lyrel('pane', 'ls', '--all')).panes;
  const panes = {};
  for (const [side, id] of Object.entries({ left, right, anchor })) {
    const row = listed.find(pane => pane.pane === id);
    assert(row?.cwd && cwdKey(row.cwd) === cwdKey(paths[side]), `${side} 的目录没有保存在布局中`);
    const live = registered.some(pane => pane.pane === id && pane.live);
    let pid;
    if (live) {
      const procs = (await lyrel('procs', `%${id}`)).procs;
      const shell = procs.find(proc => proc.depth === 0 && /^bash(?:\.exe)?$/i.test(proc.name));
      assert(shell?.pid, `未找到 %${id} 的 Bash PID`);
      pid = shell.pid;
    }
    panes[side] = { id, live, pid };
  }
  return { guiPid: gui.child.pid, tree, daemon: await lyrel('status'), panes, split: { axis: split.axis, ratio: split.ratio } };
}

async function verifyOutput(state, tokens) {
  for (const side of fixture) {
    const id = state.panes[side].id;
    const text = (await lyrel('capture', `%${id}`, '--plain', '--scrollback')).text;
    const lines = text.split(/\r?\n/).map(line => line.trim());
    for (const [token, expected] of Object.entries(tokens)) {
      assert.deepEqual(lines.filter(line => line.startsWith(`R434:${token}:`)),
        expected === side ? [`R434:${token}:${side}`] : [], `窗格 ${side} 的标记 ${token} 不匹配`);
    }
    await writeFile(join(root, `${stage}-${side}.txt`), text);
  }
  for (const token of Object.keys(tokens)) assert(await receipt(token), `缺少命令 ${token} 的 Shell 回执`);
}

function verifyTree(before, after, mode) {
  assert.deepEqual(after.split, before.split);
  assert.deepEqual(after.tree.tabs.map(tab => tab.id), before.tree.tabs.map(tab => tab.id));
  assert.equal(after.tree.active_tab, after.tree.tabs[mode === 'sleep' ? 1 : 0].id);
  assert.equal(Boolean(after.tree.tabs[0].hibernated), mode === 'sleep');
  assert.equal(Boolean(after.tree.tabs[1].hibernated), false);
  assert.equal(Boolean(after.tree.attachment), mode !== 'detached');
  if (mode === 'cold') assert.notEqual(after.daemon.pid, before.daemon.pid);
  else assert.equal(after.daemon.pid, before.daemon.pid);
  for (const side of fixture) {
    const old = before.panes[side];
    const current = after.panes[side];
    if (mode === 'sleep' && side !== 'anchor') {
      assert.equal(current.live, false);
      assert.equal(current.id, old.id);
    } else if (mode === 'cold' || (mode === 'wake' && side !== 'anchor')) {
      assert(current.live && current.pid && current.id !== old.id && current.pid !== old.pid,
        `${side} 没有得到新的活窗格和 Shell`);
    } else {
      assert(current.live && current.pid === old.pid && current.id === old.id,
        `${side} 的窗格或 Shell 没有跨 ${mode} 保留`);
    }
  }
}

async function stopDaemon() {
  const stopped = await lyrel('server', 'stop');
  assert(stopped.stopped || stopped.running === false, '隔离 daemon 未停止');
  assert(!stopped.stranded, '隔离 daemon 在停止前已无响应，不能作为正常关停样本');
}

const seeds = { seedleft: 'left', seedright: 'right', seedanchor: 'anchor' };
const tab1 = windows ? 'Alt+1' : '⌘1';

async function initialSetup() {
  console.log('请在本次新开的窗口检查操作对象。桌面输入使用键盘，控制终端只在脚本提示时使用。');
  gui = await launch(paths.left);
  console.log('图形窗口的左窗格可用后，输入 b434 seedleft 并按 Enter。');
  const left = await accepted('seedleft', 'left');
  workspace = left.owner;
  console.log(`使用 ${windows ? 'Ctrl+Shift+D' : '⌘D'} 向右分屏，右窗格输入 cd ../right && b434 seedright 并按 Enter。`);
  const right = await accepted('seedright', 'right');
  console.log(`使用 ${windows ? 'Ctrl+Shift+T' : '⌘T'} 新建标签页，输入 cd ../anchor && b434 seedanchor 并按 Enter。`);
  const anchor = await accepted('seedanchor', 'anchor');
  const tree = await until('桌面同步两个标签页', async () => {
    const state = await lyrel('ws', 'tree', workspace);
    return state.tabs.length === 2 && state.tabs[0].root.Split && state.tabs[1].root.Leaf && state;
  });
  assert.deepEqual(tree.tabs.flatMap(tab => paneIds(tab.root)), [left.pane, right.pane, anchor.pane]);
  await lyrel('ws', 'rename', workspace, 'Workspace434');
  await lyrel('tab', 'rename', `@${tree.tabs[0].id}`, 'Restore434');
  await lyrel('tab', 'rename', `@${tree.tabs[1].id}`, 'Anchor434');
  await ask(`回到桌面使用 ${tab1}，明确把焦点放到左窗格，核对两边的标记和标签页名称。然后在此按 Enter。`);
  const state = await snapshot();
  verifyTree(state, state, 'attached');
  await verifyOutput(state, seeds);
  await event('snapshot', state);
  return state;
}

async function closeWindow(before, stayResident = false) {
  await ask('关闭本次图形窗口并回到控制终端按 Enter。');
  if (!stayResident) await waitForExit(gui);
  else assert(alive(gui.child.pid), '关闭窗口时托盘没有保留 GUI 进程');
  await until('工作区解除附着', async () => !(await lyrel('ws', 'tree', workspace)).attachment);
  const state = await snapshot();
  verifyTree(before, state, 'detached');
  await event('detached', { stayResident, ...state });
  await delay(steadyMs, undefined, { signal: abort.signal });
  await event('steady', { state: 'detached' });
  return state;
}

async function result(before, mode, token, ack) {
  await ask(`核对 Restore434 左右窗格、左侧 R434:${token}:left 和两窗格原标记，保存画面后在此按 Enter。`);
  await event('operator-verified-window', { token });
  const state = await snapshot();
  assert.equal(state.panes.left.id, ack.pane, '桌面回执没有来自布局中的左窗格');
  verifyTree(before, state, mode);
  await verifyOutput(state, { ...seeds, [token]: 'left' });
  await event('snapshot', state);
  await delay(steadyMs, undefined, { signal: abort.signal });
  await event('steady', { state: 'attached' });
  return state;
}

async function restoreWindow(before, mode, index) {
  stage = `${mode}-${index}`;
  await closeWindow(before, mode === 'reattach');
  if (mode === 'cold') {
    await stopDaemon();
    await until('旧 daemon 和窗格 Shell 退出', () =>
      !alive(before.daemon.pid) && Object.values(before.panes).every(pane => !alive(pane.pid)));
    await event('daemon-stopped', { pid: before.daemon.pid });
  }
  const token = `${mode}${index}`;
  await ask(`按 Enter 启动。提示符出现时直接键入 b434 ${token} 并按 Enter，避免先点击任何窗格。`);
  const invoked = await launch();
  if (mode !== 'reattach') {
    gui = invoked;
    assert.notEqual(gui.child.pid, before.guiPid, '新启动没有创建 GUI 进程');
  }
  const ack = await accepted(token, 'left', invoked.start);
  if (mode === 'reattach') {
    await waitForExit(invoked);
    assert(alive(gui.child.pid), '重附着没有保留 GUI 进程');
  }
  return result(before, mode, token, ack);
}

async function wake(before, index) {
  stage = `wake-${index}`;
  const holdToken = `hold${index}`;
  console.log(`在 Restore434 按 F8 休眠。切换到 Anchor434 后直接输入 b434 ${holdToken} 并按 Enter。`);
  const ack = await accepted(holdToken, 'anchor');
  assert.equal(ack.pane, before.panes.anchor.id);
  await until('原 split Shell 停止且休眠布局已同步', async () => {
    const tree = await lyrel('ws', 'tree', workspace);
    return tree.tabs[0].hibernated && !alive(before.panes.left.pid) && !alive(before.panes.right.pid);
  });
  const sleeping = await snapshot();
  verifyTree(before, sleeping, 'sleep');
  await event('sleeping', sleeping);
  await delay(steadyMs, undefined, { signal: abort.signal });
  await event('steady', { state: 'sleeping' });
  const token = `wake${index}`;
  await ask(`按 Enter 开始计时，切回隔离窗口并使用 ${tab1} 唤醒。提示符出现时直接输入 b434 ${token} 并按 Enter。`);
  const start = now();
  await event('wake-request', { hrtimeMs: start, source: 'operator-start-before-desktop-shortcut' });
  return result(before, 'wake', token, await accepted(token, 'left', start));
}

try {
  await event('metadata', {
    revision: options.revision, profile: 'Release (operator supplied)', cycles, steadyMs, pollMs,
    node: process.version, app, cli, bash, appSha256: await digest(app), cliSha256: await digest(cli),
    platform: process.platform, release: release(), architecture: arch(), cpus: cpus().length,
    cpu: cpus()[0]?.model, memoryBytes: totalmem(), argv: process.argv.slice(2),
    isolation: { configDir, fixtureHome, temp, loginShell },
  });
  let before = await initialSetup();
  for (let i = 1; i <= cycles; i++) {
    before = await restoreWindow(before, 'warm', i);
    before = await restoreWindow(before, 'cold', i);
    before = await wake(before, i);
  }
  stage = 'tray-setup';
  await closeWindow(before);
  config.show_tray_icon = true;
  await writeConfig();
  await ask('按 Enter 启动托盘模式。确认本次窗口提示符出现后，直接输入 b434 traysetup。');
  gui = await launch();
  before = await result(before, 'warm', 'traysetup', await accepted('traysetup', 'left'));
  for (let i = 1; i <= cycles; i++) before = await restoreWindow(before, 'reattach', i);
  succeeded = true;
} catch (error) {
  await event('failure', { message: error.message });
  throw error;
} finally {
  input.close();
  process.off('SIGINT', interrupt);
  process.off('SIGTERM', interrupt);
  try {
    for (const instance of children) instance.child.kill();
    await Promise.all([...children].map(instance => instance.exited));
    if (launched) await stopDaemon();
  } catch (error) {
    await event('cleanup-failure', { message: error.message });
    succeeded = false;
    throw error;
  } finally {
    await event('finish', { succeeded });
    console.log(`证据保留在 ${root}`);
  }
}
