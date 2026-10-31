import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

if (process.platform !== 'linux' || !process.stdin.isTTY) throw new Error('requires the real Linux daemon PTY');
const nested = process.argv.includes('--nested');
const desktopArg = process.argv.indexOf('--desktop-path');
const desktopPath = desktopArg < 0 ? undefined : process.argv[desktopArg + 1];
const root = path.dirname(fileURLToPath(import.meta.url));
const basename = desktopPath ? path.basename(desktopPath.replaceAll('\\', '/')) : `tty-graphics-protocol-${process.pid}.rgba`;
const tempRoot = path.join(root, `media-temp-${process.pid}`);
fs.mkdirSync(tempRoot, { mode: 0o700 });
const regular = path.join(root, basename);
const temporary = path.join(tempRoot, basename);
const shmName = `/${basename}`;
const shm = `/dev/shm${shmName}`;
const pixels = Buffer.from([12, 34, 56, 255]);
const created = [];
let buffered = Buffer.alloc(0);
let wake;
process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', bytes => { buffered = Buffer.concat([buffered, bytes]); wake?.(); });
async function take(ending) {
  const deadline = Date.now() + 90000;
  for (;;) {
    const index = buffered.indexOf(ending);
    if (index >= 0) { const result = buffered.subarray(0, index + ending.length); buffered = buffered.subarray(index + ending.length); return result; }
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { wake = undefined; reject(new Error('remote media PTY deadline')); }, Math.max(0, deadline - Date.now()));
      wake = () => { clearTimeout(timer); wake = undefined; resolve(); };
    });
  }
}
async function out(bytes) {
  if (!process.stdout.write(bytes)) await new Promise(resolve => process.stdout.once('drain', resolve));
}
async function request(id, controls, payload, expected = 'OK') {
  await out(`\x1b_Gi=${id},${controls};${Buffer.from(payload).toString('base64')}\x1b\\`);
  const reply = (await take(Buffer.from('\x1b\\'))).toString('latin1');
  if (!reply.startsWith(`\x1b_Gi=${id}`) || !(expected === 'OK' ? reply.endsWith(';OK\x1b\\') : reply.includes(`;${expected}:`))) throw new Error(`remote media ${id}: ${JSON.stringify(reply)}`);
}
try {
  for (const name of [regular, temporary, shm]) { fs.writeFileSync(name, pixels, { flag: 'wx', mode: 0o600 }); created.push(name); }
  await out(`\r\nKITTY_READY\r\nKITTY_MEDIA_OBJECTS ${JSON.stringify({ regular, temporary, shmName, shm, nested })}\r\n`);
  if ((await take(Buffer.from('\n'))).toString().trim() !== 'RUN') throw new Error('expected RUN');
  const expected = nested ? 'ENOTSUPPORTED' : 'OK';
  for (const [index, medium, name] of [[1, 'f', regular], [2, 't', temporary], [3, 's', shmName]]) {
    await out(`\x1b[${4 + index * 3};5H`);
    await request(1470 + index, `a=T,p=${1470 + index},C=1,c=4,r=2,t=${medium},f=32,s=1,v=1,S=4`, name, expected);
  }
  if (!fs.existsSync(regular) || fs.existsSync(temporary) !== nested || fs.existsSync(shm) !== nested) throw new Error('host-local file/temp/shm lifecycle mismatch');
  // An opaque desktop path must never cause remote or local deletion. On a
  // Unix desktop the per-test temp file is deliberately absent on Linux.
  if (desktopPath) for (const medium of ['f', 't', 's']) await request(1480, `a=q,t=${medium},f=32,s=1,v=1,S=4`, desktopPath, nested ? 'ENOTSUPPORTED' : 'EBADF');
  await out('\x1b[17;5H');
  await request(1474, 'a=T,p=1474,C=1,c=4,r=2,f=32,s=1,v=1', Buffer.from([255, 0, 255, 255]));
  await out('\r\nKITTY_REMOTE_MEDIA_OK\r\n');
  if ((await take(Buffer.from('\n'))).toString().trim() !== 'QUIT') throw new Error('expected QUIT');
} finally {
  process.stdin.setRawMode(false);
  process.stdin.pause();
  for (const name of created) fs.rmSync(name, { force: true });
  fs.rmdirSync(tempRoot);
}
