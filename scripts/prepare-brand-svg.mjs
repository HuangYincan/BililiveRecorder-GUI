import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';

const source = new URL('../src-tauri/branding/upstream-logo.svg', import.meta.url);
const output = new URL('../src-tauri/branding/icon-white.svg', import.meta.url);
const original = readFileSync(source);
const expectedSha256 = 'd3d03458c3daecab6444f1f0654543ec2dd343db3448bdfa4f57ebe4c3c1dce0';
if (createHash('sha256').update(original).digest('hex') !== expectedSha256) {
  throw new Error('The pinned official logo SVG changed; review provenance before generating icons');
}
const svg = original.toString('utf8');
if (!svg.includes('viewBox="0 0 1024 1024"') || (svg.match(/<path\b/g) || []).length !== 4) {
  throw new Error('Unexpected official SVG structure; refusing to alter the artwork');
}
// Add only a full-canvas background before the original artwork: neither
// original path data nor its blue/off-white palette is redrawn or replaced.
const icon = svg.replace(/(<svg\b[^>]*>)/, '$1\n  <rect x="0" y="0" width="1024" height="1024" fill="#ffffff"/>');
if (icon === svg) throw new Error('SVG root element was not found');
writeFileSync(output, icon);
console.log(`White-background source prepared from official logo SHA-256 ${expectedSha256}`);
