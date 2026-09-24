import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { inflateSync } from 'node:zlib';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const iconRoot = join(root, 'src-tauri/icons');
const read = (path) => readFileSync(join(root, path));
const source = read('src-tauri/branding/upstream-logo.svg');
const derived = read('src-tauri/branding/icon-white.svg').toString('utf8');
const sourceHash = createHash('sha256').update(source).digest('hex');
if (sourceHash !== 'd3d03458c3daecab6444f1f0654543ec2dd343db3448bdfa4f57ebe4c3c1dce0') {
  throw new Error('The official logo does not match its pinned upstream SHA-256');
}
const expected = source.toString('utf8').replace(/(<svg\b[^>]*>)/,
  '$1\n  <rect x="0" y="0" width="1024" height="1024" fill="#ffffff"/>');
if (derived !== expected) throw new Error('The icon source must add only a white background to the official SVG');

const config = JSON.parse(read('src-tauri/tauri.conf.json'));
const pkg = JSON.parse(read('package.json'));
if (config.productName !== 'Mikufans录播姬' || config.identifier !== 'org.danmuji.bililiverecorder-gui') {
  throw new Error('Product name or existing updater/data identity changed unexpectedly');
}
if (config.version !== pkg.version) throw new Error('Desktop versions disagree');
if (config.bundle.windows.wix.upgradeCode !== '3a5e8872-ebd5-524f-9fe9-825771b0859b') {
  throw new Error('MSI must retain the previously shipped WiX UpgradeCode');
}
const nsis = read('src-tauri/branding/installer.nsi').toString('utf8');
for (const marker of [
  '!define LEGACYPRODUCTNAME "BililiveRecorder GUI"',
  '!define UNINSTKEY "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${LEGACYPRODUCTNAME}"',
  '!define MANUPRODUCTKEY "${MANUKEY}\\${LEGACYPRODUCTNAME}"',
  'IsShortcutTarget "$SMPROGRAMS\\${LEGACYPRODUCTNAME}.lnk"',
  'IsShortcutTarget "$DESKTOP\\${LEGACYPRODUCTNAME}.lnk"',
]) if (!nsis.includes(marker)) throw new Error(`Lost legacy NSIS update identity: ${marker}`);
if (config.bundle.windows.nsis.template !== 'branding/installer.nsi') {
  throw new Error('Brand-safe NSIS migration template is not configured');
}
const expectedPubkey = '953ea91bb74e8c0604dd78f051b9e17e8c9498a773facba64a4817ae93ecc4d5';
const pubkeyHash = createHash('sha256').update(config.plugins.updater.pubkey).digest('hex');
if (pubkeyHash !== expectedPubkey) throw new Error('Updater public signing identity changed');
if (config.plugins.updater.endpoints[0] !==
  'https://github.com/huangyincan/BililiveRecorder-GUI/releases/latest/download/latest.json') {
  throw new Error('Existing updater endpoint changed');
}

function decodePng(data) {
  if (!data.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'))) throw new Error('Invalid PNG');
  let width = 0, height = 0, channels = 0;
  const parts = [];
  for (let at = 8; at < data.length;) {
    const length = data.readUInt32BE(at);
    const type = data.toString('ascii', at + 4, at + 8);
    if (type === 'IHDR') {
      width = data.readUInt32BE(at + 8);
      height = data.readUInt32BE(at + 12);
      channels = data[at + 17] === 6 ? 4 : data[at + 17] === 2 ? 3 : 0;
      if (data[at + 16] !== 8 || !channels || data[at + 20] !== 0) throw new Error('Unsupported PNG format');
    }
    if (type === 'IDAT') parts.push(data.subarray(at + 8, at + 8 + length));
    at += 12 + length;
    if (type === 'IEND') break;
  }
  const raw = inflateSync(Buffer.concat(parts));
  const pixels = Buffer.alloc(width * height * channels);
  const stride = width * channels;
  let pos = 0, blue = 0;
  for (let y = 0; y < height; y++) {
    const filter = raw[pos++];
    for (let i = 0; i < stride; i++) {
      const left = i >= channels ? pixels[y * stride + i - channels] : 0;
      const above = y ? pixels[(y - 1) * stride + i] : 0;
      const aboveLeft = y && i >= channels ? pixels[(y - 1) * stride + i - channels] : 0;
      let prediction = 0;
      if (filter === 1) prediction = left;
      else if (filter === 2) prediction = above;
      else if (filter === 3) prediction = (left + above) >> 1;
      else if (filter === 4) {
        const p = left + above - aboveLeft;
        const d = [Math.abs(p - left), Math.abs(p - above), Math.abs(p - aboveLeft)];
        prediction = d[0] <= d[1] && d[0] <= d[2] ? left : d[1] <= d[2] ? above : aboveLeft;
      } else if (filter !== 0) throw new Error(`Unsupported PNG filter ${filter}`);
      pixels[y * stride + i] = (raw[pos++] + prediction) & 255;
    }
  }
  for (let i = 0; i < pixels.length; i += channels) {
    if (channels === 4 && pixels[i + 3] !== 255) throw new Error('Transparent pixel in desktop app icon');
    if (pixels[i] < 120 && pixels[i + 1] > 100 && pixels[i + 2] > 140) blue++;
  }
  const corner = (x, y) => [...pixels.subarray((y * width + x) * channels, (y * width + x) * channels + 3)];
  for (const [x, y] of [[0, 0], [width - 1, 0], [0, height - 1], [width - 1, height - 1]]) {
    if (corner(x, y).some((value) => value !== 255)) throw new Error('Desktop icon lacks white canvas');
  }
  if (blue < width * height / 200) throw new Error('The original blue logo is missing');
  return { width, height, blue };
}

for (const name of ['32x32.png','64x64.png','128x128.png','128x128@2x.png','icon.png',
  ...readdirSync(iconRoot).filter((name) => /^(Square.*Logo|StoreLogo)\.png$/.test(name))]) {
  const result = decodePng(readFileSync(join(iconRoot, name)));
  console.log(`White/opaque branded icon ${name} ${result.width}x${result.height}`);
}
const ico = readFileSync(join(iconRoot, 'icon.ico'));
let icoFrames = 0;
for (let i = 0; i < ico.readUInt16LE(4); i++) {
  const offset = 6 + i * 16;
  const size = ico.readUInt32LE(offset + 8);
  const start = ico.readUInt32LE(offset + 12);
  decodePng(ico.subarray(start, start + size));
  icoFrames++;
}
if (icoFrames < 4) throw new Error('Windows ICO lacks size variants');
const icns = readFileSync(join(iconRoot, 'icon.icns'));
if (icns.toString('ascii', 0, 4) !== 'icns') throw new Error('Missing macOS ICNS');
let icnsPngs = 0;
for (let at = 8; at < icns.length;) {
  const size = icns.readUInt32BE(at + 4);
  if (size < 8) throw new Error('Invalid ICNS chunk');
  const png = icns.subarray(at + 8, at + size);
  if (png.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'))) {
    decodePng(png);
    icnsPngs++;
  }
  at += size;
}
if (icnsPngs < 4) throw new Error('macOS ICNS lacks PNG variants');
console.log(`Identity kept; ${icoFrames} ICO and ${icnsPngs} ICNS frames use opaque white official artwork`);
