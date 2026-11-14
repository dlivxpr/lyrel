// 在隔离候选 pane 中执行：node scripts/terminal-capabilities.mjs
// 跨 detach/reconnect 场景：node scripts/terminal-capabilities.mjs --reconnect
// 不读取或修改 lyrel 配置；只通过当前真实终端的输入/输出进行协议查询。
// XTGETTCAP 只回答 Co/colors/RGB；静态 Sixel 的发现走 DA1 属性 4 与
// XTSMGRAPHICS，两者都由实际连接的 endpoint 能力决定。`sixel` 作为
// XTGETTCAP 名称仍是负答，因为它不是 terminfo 能力名，而不是因为图像不支持。
import assert from 'node:assert/strict';

if (!process.stdin.isTTY || !process.stdout.isTTY) {
  throw new Error('需要真实终端 TTY；请在候选 lyrel pane 中执行');
}
const reconnect = process.argv.includes('--reconnect');
const esc = '\x1b';
const st = `${esc}\\`;
const request = (names) => `${esc}P+q${names.map((name) => Buffer.from(name).toString('hex')).join(';')}${st}`;
const positive = (name, value) => `${esc}P1+r${Buffer.from(name).toString('hex').toUpperCase()}=${Buffer.from(value).toString('hex').toUpperCase()}${st}`;
const negative = (name) => `${esc}P0+r${Buffer.from(name).toString('hex').toUpperCase()}${st}`;
const barrier = `${esc}_Gi=4294967294;ENOTSUPPORTED:Unsupported operation${st}`;
const barrierQuery = `${esc}_Ga=q,i=4294967294,f=7${st}`;
let pending;
let stray = '';
const onData = (bytes) => {
  const text = bytes.toString('latin1');
  if (text.includes('\x03')) {
    pending?.reject(new Error('用户中止'));
    return;
  }
  if (pending) {
    pending.bytes += text;
    const end = pending.finish(pending.bytes);
    if (end !== null) {
      const probe = pending;
      pending = undefined;
      clearTimeout(probe.timer);
      stray += probe.bytes.slice(end);
      probe.resolve(probe.bytes.slice(0, end));
    }
  } else {
    stray += text;
  }
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function pendingProbe(label, finish) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      const received = pending?.bytes;
      pending = undefined;
      reject(new Error(`${label} 超时，实际输入 ${JSON.stringify(received)}`));
    }, 10000);
    pending = { bytes: '', resolve, reject, timer, finish };
  });
}
async function probe(label, bytes, expected, fragment = false) {
  assert.equal(stray, '', `非查询时收到额外输入：${JSON.stringify(stray)}`);
  const result = pendingProbe(label, (bytes) => {
    const at = bytes.indexOf(barrier);
    return at < 0 ? null : at + barrier.length;
  });
  if (fragment) {
    for (const byte of bytes) {
      process.stdout.write(byte);
      await sleep(2);
    }
  } else {
    process.stdout.write(bytes);
  }
  process.stdout.write(barrierQuery);
  assert.equal(await result, expected + barrier, label);
  process.stdout.write(`\r\nPASS ${label}\r\n`);
}
// A probe whose owner answers without a Kitty barrier: DA1 and XTSMGRAPHICS are
// plain CSI queries, so completion is the expected bytes themselves. Anything
// extra stays in `stray` and fails the next probe, which catches duplicate or
// replayed answers.
async function probeAnswer(label, bytes, expected) {
  assert.equal(stray, '', `非查询时收到额外输入：${JSON.stringify(stray)}`);
  const result = pendingProbe(label, (bytes) =>
    bytes === expected ? bytes.length : null,
  );
  process.stdout.write(bytes);
  assert.equal(await result, expected, label);
  process.stdout.write(`\r\nPASS ${label}\r\n`);
}
// Same ownership as `probeAnswer`, for answers whose value is the live pane
// geometry rather than a constant: complete on the first full match, and
// report the observed bytes so the scene records the actual size.
async function probePattern(label, bytes, pattern) {
  assert.equal(stray, '', `非查询时收到额外输入：${JSON.stringify(stray)}`);
  const result = pendingProbe(label, (bytes) =>
    pattern.test(bytes) ? bytes.length : null,
  );
  process.stdout.write(bytes);
  const observed = await result;
  assert.match(observed, pattern, label);
  process.stdout.write(`\r\nPASS ${label} ${JSON.stringify(observed)}\r\n`);
  return observed;
}
const wasRaw = process.stdin.isRaw;
process.stdin.setRawMode(true);
process.stdin.on('data', onData);
process.stdin.resume();
try {
  process.stdout.write(`\r\nXTGETTCAP scene Node ${process.version}; TERM=${process.env.TERM}; TERM_PROGRAM=${process.env.TERM_PROGRAM}\r\n`);
  await probe('allowed + denied ordered batch', request(['Co', 'TN', 'RGB', 'Ms', 'sixel', 'iterm2', 'cols', 'lines', 'Glyph', 'unknown']),
    positive('Co', '256') + negative('TN') + positive('RGB', '8/8/8') + ['Ms', 'sixel', 'iterm2', 'cols', 'lines', 'Glyph', 'unknown'].map(negative).join(''));
  await probe('fragmented request and ST', request(['colors']), positive('colors', '256'), true);
  await probe('malformed hex', `${esc}P+qxyz${st}`, `${esc}P0+r${st}`);
  await probe('CAN cancellation', `${esc}P+q436f\x18ordinary text\r\n`, '');
  await probe('SUB cancellation', `${esc}P+q436f\x1aordinary text\r\n`, '');
  await probe('interrupted DCS', `${esc}P+q436f${esc}[0mordinary text\r\n`, '');
  if (reconnect) {
    process.stdout.write('\r\n请在接下来 15 秒内关闭候选窗口（保留 daemon），再重新打开；后台仍将查询。\r\n');
    for (let round = 0; round < 15; round++) {
      await sleep(1000);
      await probe(`attach/detach/reconnect ${round + 1}`, request(['Co', 'unknown']), positive('Co', '256') + negative('unknown'));
    }
  }
  await probe('no historical/duplicate replies', request(['RGB']), positive('RGB', '8/8/8'));
  await probe('oversized request followed by ordinary text', `${esc}P+q${'436f;'.repeat(2000)}${st}after oversized request\r\n`, '');
  await probe('legal query after oversize', request(['Co']), positive('Co', '256'));
  // Sixel 没有 terminfo 能力名，因此 XTGETTCAP 对 `sixel` 的负答保持真实；
  // 静态 Sixel 的发现路径是 DA1 属性 4 与 XTSMGRAPHICS。两项都由实际
  // endpoint 回答：DA1 由附着客户端按协商能力回答，XTSMGRAPHICS 由持有
  // 像素几何与准入上限的 daemon 回答。
  await probeAnswer('DA1 advertises the static Sixel attribute', `${esc}[c`, `${esc}[?4;6c`);
  await probeAnswer('XTSMGRAPHICS color registers', `${esc}[?1;1;0S`, `${esc}[?1;0;1024S`);
  // 具体像素值取决于本 pane 的实际单元尺寸与行列数。
  await probePattern(
    'XTSMGRAPHICS current geometry',
    `${esc}[?2;1;0S`,
    /^\x1b\[\?2;0;\d{1,4};\d{1,4}S$/,
  );
  await probeAnswer('XTSMGRAPHICS denies ReGIS', `${esc}[?3;1;0S`, `${esc}[?3;1S`);
  await probeAnswer('XTSMGRAPHICS denies a rectangular maximum', `${esc}[?2;4S`, `${esc}[?2;3S`);
  process.stdout.write('\r\nXTGETTCAP/Sixel discovery scene passed; 此脚本不是 Neovim/notcurses 消费者证据。\r\n');
} finally {
  if (pending) clearTimeout(pending.timer);
  process.stdin.off('data', onData);
  process.stdin.setRawMode(Boolean(wasRaw));
  process.stdin.pause();
}
