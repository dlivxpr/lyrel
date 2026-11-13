// Run inside an isolated candidate lyrel pane: node tests/support/pointer_scene.mjs
// No terminal identity overrides or capability queries are sent.
import process from 'node:process';

const osc = (payload) => `\x1b]22;${payload}\x1b\\`;
const write = (bytes) => process.stdout.write(bytes);
const names = { '1': 'pointer', '2': 'ew-resize', '3': 'ne-resize', '4': 'grab', '5': 'help', '0': '' };
function scene() {
  write('\x1b[2J\x1b[H');
  write('OSC 22 native scene: basic set/reset only\r\n');
  write(`TERM=${process.env.TERM ?? ''} TERM_PROGRAM=${process.env.TERM_PROGRAM ?? ''}\r\n`);
  write(`TMUX=${process.env.TMUX ?? ''} STY=${process.env.STY ?? ''} platform=${process.platform}\r\n`);
  write('Move: left=pointer, middle=ew-resize, right=ne-resize. Drag row 13 divider.\r\n');
  write('Shift+drag selects text; Ctrl/Cmd+click opens the OSC 8 link below.\r\n');
  write('\x1b]8;;https://example.org/pointer\x1b\\https://example.org/pointer\x1b]8;;\x1b\\\r\n');
  write('Keys: 1 pointer | 2 ew-resize | 3 ne-resize | 4 grab | 5 help (native fallback)\r\n');
  write('0 empty reset | f fragmented reset | m toggle mouse | r RIS | d command-finished\r\n');
  write('q returns to shell without explicit OSC 22 reset; Ctrl+C also exits.\r\n');
  drawDivider();
}
if (!process.stdin.isTTY || !process.stdout.isTTY) {
  throw new Error('Run this scene in a real terminal PTY, not redirected input/output');
}
process.stdin.setRawMode(true);
process.stdin.setEncoding('utf8');
let mouse = true;
let buffered = '';
let claimed = null;
let fragmentPending = false;
let divider = 40;
let dragging = false;
function drawDivider() {
  const width = Math.max(4, Math.min(process.stdout.columns || 80, 160));
  divider = Math.max(2, Math.min(divider, width - 2));
  write(`\x1b[13;1H\x1b[2K${'L'.repeat(divider - 1)}|${'R'.repeat(width - divider - 1)}`);
}
function claim(name) {
  if (claimed !== name) {
    write(osc(name));
    claimed = name;
  }
}
function mouseMode() {
  write(mouse ? '\x1b[?1003h\x1b[?1006h' : '\x1b[?1003l\x1b[?1006l');
}
function finish() {
  write('\x1b[?1003l\x1b[?1006l\r\nScene exited; shell owns pointer cleanup.\r\n');
  process.stdin.setRawMode(false);
  process.exit(0);
}
scene();
mouseMode();
process.stdin.on('data', (chunk) => {
  if (fragmentPending) return;
  buffered += chunk;
  while (buffered.length) {
    if (buffered.startsWith('\x1b[<')) {
      const report = /^\x1b\[<(\d+);(\d+);(\d+)([Mm])/.exec(buffered);
      if (!report) {
        if (buffered.length < 64) return;
        buffered = '';
        return;
      }
      buffered = buffered.slice(report[0].length);
      const [, button, x, y, action] = report;
      const column = Number(x);
      const code = Number(button);
      if (action === 'M' && code === 0 && Number(y) === 13 && Math.abs(column - divider) <= 1) dragging = true;
      if (dragging && (code & 32)) { divider = column; drawDivider(); }
      claim(dragging ? 'ew-resize' : column < 26 ? 'pointer' : column < 52 ? 'ew-resize' : 'ne-resize');
      write(`\x1b[11;1H\x1b[2Kmouse button=${button} x=${x} y=${y} shape=${claimed} divider=${divider}`);
      if (action === 'm') dragging = false;
      continue;
    }
    if ('\x1b[<'.startsWith(buffered)) return;
    const key = buffered[0];
    buffered = buffered.slice(1);
    if (key === 'q' || key === '\x03') return finish();
    if (Object.hasOwn(names, key)) claim(names[key]);
    if (key === 'm') { mouse = !mouse; mouseMode(); }
    if (key === 'd') { write('\x1b]133;D;0\x07'); claimed = null; }
    if (key === 'r') { write('\x1bc'); claimed = null; scene(); mouseMode(); }
    if (key === 'f') {
      fragmentPending = true;
      write('\x1b]22;\x1b');
      // Do not insert hover output into an unfinished OSC sequence.
      setTimeout(() => {
        write('\\FRAGMENT-RESET-DONE\r\n');
        claimed = '';
        fragmentPending = false;
      }, 50);
      buffered = '';
      return;
    }
  }
});
