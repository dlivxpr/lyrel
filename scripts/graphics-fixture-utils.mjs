// Test-only PNG encoding and bounded terminal output shared by native fixtures.
import { once } from 'node:events';
import { deflateSync } from 'node:zlib';

function pngChunk(type, data) {
  const name = Buffer.from(type);
  let crc = 0xffffffff;
  for (const bytes of [name, data]) for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  const length = Buffer.alloc(4); length.writeUInt32BE(data.length);
  const checksum = Buffer.alloc(4); checksum.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
  return Buffer.concat([length, name, data, checksum]);
}

// One synthetic raster for every native picture fixture: four quadrants plus a
// white centre cross. `index` selects the revision. Returns the zero-filter
// RGBA scanlines `encodePng` consumes and the raw RGBA identity.
export function fixturePicture(index) {
  const width = 48, height = 24;
  const rgba = Buffer.alloc(width * height * 4);
  const scan = Buffer.alloc(height * (width * 4 + 1));
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const color = x >= 22 && x < 26 && y >= 10 && y < 14 ? [255, 255, 255]
        : y < 12 ? x < 24 ? [255, 0, index * 11] : [0, 255, index * 7]
        : x < 24 ? [index * 5, 0, 255] : [255, 255, 0];
      rgba.set([...color, 255], (y * width + x) * 4);
    }
    rgba.copy(scan, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  }
  return { width, height, scan, rgba };
}

// Callers retain their own picture identities and zero-filter RGBA scanlines.
export function encodePng(width, height, scanlines) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width); header.writeUInt32BE(height, 4);
  header[8] = 8; header[9] = 6;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    pngChunk('IHDR', header), pngChunk('IDAT', deflateSync(scanlines)), pngChunk('IEND', Buffer.alloc(0))]);
}

// Raw buffers are encoded lazily: 3072 raw bytes produce at most 4096 Base64 bytes.
export function* base64Chunks(data) {
  const raw = Buffer.isBuffer(data);
  const size = raw ? 3072 : 4096;
  for (let offset = 0; offset < data.length; offset += size) {
    const end = Math.min(offset + size, data.length);
    const chunk = raw ? data.subarray(offset, end).toString('base64') : data.slice(offset, end);
    yield { chunk, first: offset === 0, more: Number(end < data.length) };
  }
}

export async function writeTerminal(text, stream = process.stdout) {
  if (!stream.write(text)) await once(stream, 'drain');
}
