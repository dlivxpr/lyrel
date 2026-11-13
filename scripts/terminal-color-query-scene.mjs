// 在自有隔离 lyrel pane 中运行：node scripts/terminal-color-query-scene.mjs
// 切换 light / dracula 后按 r；比较每行 indexed/default 与 queried RGB。
// 查询值来自真实 PTY 输入，不设置 OSC 动态颜色，也不修改应用配置。
const csi = '\x1b[';
const osc = '\x1b]';
const reset = `${csi}0m`;
const probes = ['4;1', '4;9', '4;200', '10', '11', '12'];
const answers = new Map();
let pending = '';
let timer;
let revision = 0;
if (!process.stdin.isTTY) throw new Error('Run inside an interactive terminal pane');

function draw() {
  process.stdout.write(`${reset}${csi}2J${csi}H`);
  const line = text => process.stdout.write(`${text}${reset}\r\n`);
  const rgb = key => answers.get(key)?.join(';');
  const sample = (key, original) => {
    const value = rgb(key);
    line(`${key.padEnd(6)} ${original}████ SAMPLE${reset} | ${value ? `${csi}38;2;${value}m████ SAMPLE${reset}  RGB ${value}` : 'NO REPLY (not verified)'}`);
  };
  line('LYREL #260: painted palette / real PTY query comparison');
  line('Each left sample must match its queried-RGB sample on the right.');
  line('Switch application theme (light / dracula), then press r to re-query.');
  line('Select samples and search SAMPLE; underline/dim should stay coherent.');
  sample('4;1', `${csi}31m`);
  sample('4;9', `${csi}91m`);
  sample('4;200', `${csi}38;5;200m`);
  sample('10', reset);
  const background = rgb('11');
  line(`11     default blank [          ] | queried background [${background ? `${csi}48;2;${background}m          ${reset}` : 'NO REPLY'}] RGB ${background ?? '?'}`);
  const caret = rgb('12');
  line(`12     queried caret ${caret ? `${csi}38;2;${caret}m████${reset}` : 'NO REPLY'} RGB ${caret ?? '?'}`);
  line(`${csi}31;4mANSI red underline SAMPLE${reset} | ${csi}31;2mANSI red dim SAMPLE${reset}`);
  line('r = re-query after theme switch; q / Ctrl+C = exit');
  line(`COLOR_QUERY_READY_${revision} ${answers.size}/${probes.length} replies`);
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
      const match = /^\x1b\](4;\d+|10|11|12);rgb:([\da-f]+)\/([\da-f]+)\/([\da-f]+)(?:\x07|\x1b\\)/i.exec(pending);
      if (!match) {
        // 保留分片 OSC；未知完整回复丢弃，不能当作键盘命令执行。
        const end = /\x07|\x1b\\/.exec(pending);
        if (!end) break;
        pending = pending.slice(end.index + end[0].length);
        continue;
      }
      answers.set(match[1], match.slice(2, 5).map(hex => Math.round(parseInt(hex, 16) * 255 / (16 ** hex.length - 1))));
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
      if (key === 'r') query();
    }
  }
});
process.stdout.write(`${csi}?1049h`);
query();
