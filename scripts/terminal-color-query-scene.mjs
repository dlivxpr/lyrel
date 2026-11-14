// 在自有隔离 lyrel pane 中运行：node scripts/terminal-color-query-scene.mjs
// s = 设置窗格覆盖；x = 复位；r = 重新查询；q = 退出。
// 左侧是真实绘制样本，右侧是从同一 PTY 回复解析的 RGB 样本；两者必须一致。
// 覆盖后切换应用主题：未覆盖色位应跟随主题，覆盖色位保持不变；x 后应露出当前主题。
const csi = '\x1b[';
const osc = '\x1b]';
const reset = `${csi}0m`;
const probes = ['4;1', '4;2', '10', '11', '12'];
const answers = new Map();
let pending = '';
let timer;
let revision = 0;
let override = false;
if (!process.stdin.isTTY) throw new Error('Run inside an interactive terminal pane');

const setSequence =
  `${osc}4;1;#112233\x07${osc}10;#0a0b0c\x07${osc}11;#101112\x07${osc}12;#131415\x07`;
const resetSequence = `${osc}104;1\x07${osc}110\x07${osc}111\x07${osc}112\x07`;

function draw() {
  process.stdout.write(`${reset}${csi}2J${csi}H`);
  const line = text => process.stdout.write(`${text}${reset}\r\n`);
  const rgb = key => answers.get(key)?.join(';');
  const sample = (key, original, label) => {
    const value = rgb(key);
    line(
      `${key.padEnd(6)} ${original}████ ${label}${reset} | ` +
        (value
          ? `${csi}38;2;${value}m████ ${label}${reset}  RGB ${value}`
          : 'NO REPLY (not verified)'),
    );
  };
  line('LYREL #261: pane colour override / real PTY query comparison');
  line(`state: ${override ? 'OVERRIDE (s applied)' : 'DEFAULTS (no override)'}`);
  line('Each left sample must match its queried-RGB sample on the right.');
  line('s = set override; x = reset; switch theme, then r = re-query; q = exit.');
  sample('4;1', `${csi}31m`, 'ANSI-1');
  sample('4;2', `${csi}32m`, 'ANSI-2');
  sample('10', reset, 'DEFAULT-FG');
  const background = rgb('11');
  line(
    `11     default blank [          ] | ` +
      (background
        ? `${csi}48;2;${background}m          ${reset} RGB ${background}`
        : 'NO REPLY'),
  );
  const caret = rgb('12');
  line(
    `12     queried caret ` +
      (caret ? `${csi}38;2;${caret}m████${reset} RGB ${caret}` : 'NO REPLY'),
  );
  line(`${csi}31;4mANSI red underline SAMPLE${reset} | ${csi}31;2mANSI red dim SAMPLE${reset}`);
  line(`COLOR_OVERRIDE_READY_${revision} ${answers.size}/${probes.length} replies override=${override}`);
  process.stdout.write(`${csi}6 q${csi}?25hCursor (bar) -> `);
}

function query() {
  clearTimeout(timer);
  answers.clear();
  revision++;
  process.stdout.write(probes.map(key => `${osc}${key};?\x07`).join(''));
  timer = setTimeout(draw, 2000);
}
function close() {
  clearTimeout(timer);
  process.stdout.write(`${reset}${csi}0 q${csi}?25h${csi}?1049l`);
  process.stdin.setRawMode(false);
  process.exit(0);
}
process.stdin.setRawMode(true);
process.stdin.setEncoding('utf8');
process.stdin.on('data', data => {
  pending += data;
  while (pending) {
    if (pending.startsWith('\x1b]')) {
      const match = /^\x1b\](4;\d+|10|11|12);rgb:([\da-f]+)\/([\da-f]+)\/([\da-f]+)(?:\x07|\x1b\\)/i.exec(
        pending,
      );
      if (!match) {
        const end = /\x07|\x1b\\/.exec(pending);
        if (!end) break;
        pending = pending.slice(end.index + end[0].length);
        continue;
      }
      answers.set(
        match[1],
        match
          .slice(2, 5)
          .map(hex => Math.round((parseInt(hex, 16) * 255) / (16 ** hex.length - 1))),
      );
      pending = pending.slice(match[0].length);
      if (answers.size === probes.length) {
        clearTimeout(timer);
        draw();
      }
    } else if (pending === '\x1b') {
      break;
    } else {
      const key = pending[0];
      pending = pending.slice(1);
      if (key === 'q' || key === '\x03') close();
      if (key === 's') {
        override = true;
        process.stdout.write(setSequence);
        query();
      }
      if (key === 'x') {
        override = false;
        process.stdout.write(resetSequence);
        query();
      }
      if (key === 'r') query();
    }
  }
});
process.stdout.write(`${csi}?1049h`);
query();
