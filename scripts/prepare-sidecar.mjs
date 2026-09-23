import { execFileSync } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import process from 'node:process';
import AdmZip from 'adm-zip';

const manifest = JSON.parse(readFileSync(new URL('../upstream.json', import.meta.url), 'utf8'));
const version = process.env.BILILIVE_RECORDER_VERSION || manifest.version;
const target = process.env.TAURI_TARGET_TRIPLE || execFileSync('rustc', ['--print', 'host-tuple'], { encoding: 'utf8' }).trim();

const platformAssets = [
  [/^aarch64-apple-darwin$/, 'BililiveRecorder-CLI-osx-arm64.zip'],
  [/^x86_64-apple-darwin$/, 'BililiveRecorder-CLI-osx-x64.zip'],
  [/^x86_64-pc-windows-msvc$/, 'BililiveRecorder-CLI-win-x64.zip'],
  [/^x86_64-unknown-linux-gnu$/, 'BililiveRecorder-CLI-linux-x64.zip'],
  [/^aarch64-unknown-linux-gnu$/, 'BililiveRecorder-CLI-linux-arm64.zip'],
  [/^aarch64-unknown-linux-musl$/, 'BililiveRecorder-CLI-linux-musl-arm64.zip'],
  [/^x86_64-unknown-linux-musl$/, 'BililiveRecorder-CLI-linux-musl-x64.zip'],
];

const asset = platformAssets.find(([pattern]) => pattern.test(target))?.[1];
if (!asset) throw new Error(`Unsupported target triple: ${target}`);

const extension = target.includes('windows') ? '.exe' : '';
const sidecarRoot = join('src-tauri', 'sidecar');
const outputDirectory = join(sidecarRoot, target);
const output = join(outputDirectory, `BililiveRecorder.Cli${extension}`);
if (existsSync(output) && process.env.FORCE_SIDECAR_DOWNLOAD !== '1') {
  console.log(`Sidecar already prepared: ${output}`);
  process.exit(0);
}

rmSync(outputDirectory, { recursive: true, force: true });
mkdirSync(outputDirectory, { recursive: true });
const downloadUrl = `https://github.com/BililiveRecorder/BililiveRecorder/releases/download/${version}/${asset}`;
console.log(`Downloading ${downloadUrl}`);
const response = await fetch(downloadUrl, { headers: { 'User-Agent': 'BililiveRecorder-GUI build' }, redirect: 'follow' });
if (!response.ok) throw new Error(`Download failed: ${response.status} ${response.statusText}`);

const archivePath = join(sidecarRoot, asset);
writeFileSync(archivePath, Buffer.from(await response.arrayBuffer()));
const archive = new AdmZip(archivePath);
const executableName = `BililiveRecorder.Cli${extension}`;
archive.extractAllTo(outputDirectory, true);
if (!existsSync(output)) throw new Error(`${executableName} not found at archive root in ${asset}`);
if (!extension) chmodSync(output, 0o755);
rmSync(archivePath, { force: true });
console.log(`Prepared ${output}`);
