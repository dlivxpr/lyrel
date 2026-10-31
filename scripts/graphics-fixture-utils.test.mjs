import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { inflateSync } from 'node:zlib';
import { encodePng, base64Chunks, writeTerminal } from './graphics-fixture-utils.mjs';

test('fixture PNG preserves RGBA scanlines and standard chunk checksums', () => {
  const scanlines = Buffer.from([0, 255, 0, 0, 255]);
  const png = encodePng(1, 1, scanlines);
  assert.equal(png.subarray(0, 33).toString('hex'),
    '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489');
  assert.equal(png.subarray(-12).toString('hex'), '0000000049454e44ae426082');
  assert.equal(png.subarray(37, 41).toString(), 'IDAT');
  assert.deepEqual(inflateSync(png.subarray(41, 41 + png.readUInt32BE(33))), scanlines);
});

test('Kitty batching preserves encoded padding and continuation at the 4096 boundary', () => {
  const encoded = 'A'.repeat(4096) + 'AQ==';
  assert.deepEqual([...base64Chunks(encoded)], [
    { chunk: 'A'.repeat(4096), first: true, more: 1 },
    { chunk: 'AQ==', first: false, more: 0 },
  ]);
  assert.deepEqual([...base64Chunks('AQ')], [{ chunk: 'AQ', first: true, more: 0 }]);
  assert.deepEqual([...base64Chunks('A'.repeat(4096))], [
    { chunk: 'A'.repeat(4096), first: true, more: 0 },
  ]);
  assert.deepEqual([...base64Chunks('')], []);
});

test('raw capacity batching encodes only the requested chunk without a whole-frame Base64 copy', () => {
  const pixels = Buffer.alloc(3073, 255);
  const chunks = base64Chunks(pixels);
  assert.deepEqual(chunks.next().value, { chunk: '/'.repeat(4096), first: true, more: 1 });
  pixels[3072] = 1;
  assert.deepEqual(chunks.next().value, { chunk: 'AQ==', first: false, more: 0 });
  assert.equal(chunks.next().done, true);
});

test('terminal writes wait for drain without repeating accepted bytes', async () => {
  const stream = new EventEmitter();
  const writes = [];
  stream.write = text => { writes.push(text); return false; };
  let completed = false;
  const pending = writeTerminal('picture', stream).then(() => { completed = true; });
  await Promise.resolve();
  assert.equal(completed, false);
  assert.deepEqual(writes, ['picture']);
  stream.emit('drain');
  await pending;
  assert.equal(completed, true);
  assert.deepEqual(writes, ['picture']);
});

test('terminal writes propagate stream failures while waiting for drain', async () => {
  const stream = new EventEmitter();
  stream.write = () => false;
  const pending = writeTerminal('picture', stream);
  const rejected = assert.rejects(pending, /broken fixture stream/);
  stream.emit('error', new Error('broken fixture stream'));
  await rejected;
});
