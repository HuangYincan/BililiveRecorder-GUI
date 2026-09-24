import { readFileSync, writeFileSync } from 'node:fs';

// Tauri's ICNS encoder writes correct but order-unstable chunks. Sorting the
// unchanged chunks makes `npm run brand:icons` byte-reproducible, not a redraw.
const path = new URL('../src-tauri/icons/icon.icns', import.meta.url);
const data = readFileSync(path);
if (data.toString('ascii', 0, 4) !== 'icns' || data.readUInt32BE(4) !== data.length) {
  throw new Error('Invalid generated ICNS');
}
const chunks = [];
for (let offset = 8; offset < data.length;) {
  const size = data.readUInt32BE(offset + 4);
  if (size < 8 || offset + size > data.length) throw new Error('Invalid ICNS chunk length');
  chunks.push(data.subarray(offset, offset + size));
  offset += size;
}
chunks.sort((a, b) => {
  const left = a.toString('ascii', 0, 4);
  const right = b.toString('ascii', 0, 4);
  return left < right ? -1 : left > right ? 1 : 0;
});
writeFileSync(path, Buffer.concat([data.subarray(0, 8), ...chunks]));
console.log(`Canonicalized ${chunks.length} ICNS chunks without changing artwork`);
