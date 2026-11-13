// 在隔离候选 pane 中执行：node scripts/terminal-capabilities.mjs
// 跨 detach/reconnect 场景：node scripts/terminal-capabilities.mjs --reconnect
// 不读取或修改 lyrel 配置；只通过当前真实终端的输入/输出进行协议查询。
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
    if (pending.bytes.includes(barrier)) {
      const probe = pending;
      pending = undefined;
      clearTimeout(probe.timer);
      const at = probe.bytes.indexOf(barrier);
      stray += probe.bytes.slice(at + barrier.length);
      probe.resolve(probe.bytes.slice(0, at));
    }
  } else {
    stray += text;
  }
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function probe(label, bytes, expected, fragment = false) {
  assert.equal(stray, '', `非查询时收到额外输入：${JSON.stringify(stray)}`);
  const result = new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      const received = pending?.bytes;
      pending = undefined;
      reject(new Error(`${label} 超时，实际输入 ${JSON.stringify(received)}`));
    }, 10000);
    pending = { bytes: '', resolve, reject, timer };
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
  assert.equal(await result, expected, label);
  process.stdout.write(`\r\nPASS ${label}\r\n`);
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
  process.stdout.write('\r\nXTGETTCAP scene passed; 此脚本不是 Neovim/notcurses 消费者证据。\r\n');
} finally {
  if (pending) clearTimeout(pending.timer);
  process.stdin.off('data', onData);
  process.stdin.setRawMode(Boolean(wasRaw));
  process.stdin.pause();
}
