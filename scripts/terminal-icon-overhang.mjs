// Synthetic terminal content for native icon-overhang acceptance; no user data.
const esc = '\x1b[';
const reset = `${esc}0m`;
const bg = `${esc}48;2;32;64;96m`;
const other = `${esc}48;2;128;32;48m`;
const fg = `${esc}38;2;255;0;255m`;
const icons = ['\uf002', '\uf179', '\uf059', '\uf07c'];
const labelWidth = 24;
let revision = 0;
const rows = [
  ['default blank', icon => `${icon} X `],
  ['matching background', icon => `${bg}${icon} ${reset}${fg}X `],
  ['nonblank neighbor', icon => `${bg}${icon}X${reset}${fg}X `],
  ['mismatched background', icon => `${bg}${icon}${other} ${reset}${fg}X `],
  ['icon-only background', icon => `${bg}${icon}${reset}${fg} X `],
  ['blank-only background', icon => `${icon}${bg} ${reset}${fg}X `],
  ['underlined blank', icon => `${bg}${icon}${esc}4m ${reset}${fg}X `],
  ['struck blank', icon => `${bg}${icon}${esc}9m ${reset}${fg}X `],
];
function draw() {
  process.stdout.write(`${reset}${esc}2J${esc}H${esc}?25l${fg}`);
  const line = text => process.stdout.write(text + reset + '\r\n' + fg);
  line('LYREL: controlled PUA icon / compatible blank fixture');
  line('Icons: U+F002 U+F179 U+F059 U+F07C; four cells/sample.');
  line('Use a primary Nerd Font at 22px, then an icon fallback.');
  line('Matching colored blank must preserve default ink size.');
  line('Nonblank/incompatible/decorated blanks must stay intact.');
  line('Select icon+space together, then only one cell; copy.');
  line('Keys: s = output and redraw; q = exit.');
  line(''.padEnd(labelWidth) + '1234123412341234');
  for (const [label, render] of rows) line(label.padEnd(labelWidth) + icons.map(render).join(''));
  line('COPY_CONTROL: A\uf002 X 中 e\u0301 Z');
  line(`MATRIX_READY_${revision}`);
}
process.stdout.write(`${esc}?1049h`);
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', data => {
  if (data.includes(113) || data.includes(3)) {
    process.stdout.write(`${reset}${esc}?25h${esc}?1049l`);
    if (process.stdin.isTTY) process.stdin.setRawMode(false);
    process.exit(0);
  }
  if (data.includes(115)) {
    for (let i = 0; i < 60; i++) process.stdout.write(`${reset}SCROLL_${i} \uf002 X\r\n`);
    revision++;
    draw();
  }
});
draw();
