import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { releaseAssetNames, ASSET_NAME_PATTERN } from './release-asset-names.mjs';
import { verifyUploadedReleaseAssets } from './verify-uploaded-release-assets.mjs';

const repoRoot = new URL('../', import.meta.url);
const workflow = readFileSync(new URL('../.github/workflows/release.yml', import.meta.url), 'utf8');
const observed = JSON.parse(readFileSync(new URL('./fixtures/failed-app-v2.20.2-asset-names.json', import.meta.url), 'utf8'));
const version = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).version;
const sha = 'a'.repeat(40);
const actual = (names) => ({
  tagName: `app-v${version}`, isDraft: true, targetCommitish: sha,
  assets: names.map((name) => ({ name, size: 1, digest: `sha256:${'0'.repeat(64)}` })),
});

test('all three pinned tauri-action uploads apply the same ASCII upload rule', () => {
  assert.equal((workflow.match(/assetNamePattern: MikufansRecorder_\[version]\_\[platform]\_\[arch]\[setup]\[ext]/g) || []).length, 3);
  assert.equal((workflow.match(/uses: tauri-apps\/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5/g) || []).length, 3);
  assert.equal(ASSET_NAME_PATTERN, 'MikufansRecorder_[version]_[platform]_[arch][setup][ext]');
  const gate = workflow.indexOf('Verify actual uploaded ASCII asset names');
  assert.ok(gate > 0 && gate < workflow.indexOf('Establish new tag and verify'));
  const expected = releaseAssetNames(version);
  assert.equal(expected.all.length, 17);
  assert.equal(new Set(expected.all).size, expected.all.length);
  assert.ok(expected.all.every((name) => /^[\x20-\x7e]+$/.test(name)));
  assert.deepEqual(verifyUploadedReleaseAssets(actual(expected.all), version, sha), expected.all);
});

test('real failed 2.20.2 GitHub draft asset names reproduce the Unicode normalization mismatch', () => {
  // These exact names were fetched read-only from the 17-asset failed draft,
  // not guessed from our pattern. Keep it as a regression for GitHub's upload
  // normalization that originally escaped the handcrafted manifest fixture.
  assert.equal(observed.isDraft, true);
  assert.equal(observed.tagName, 'app-v2.20.2');
  assert.equal(observed.names.length, 17);
  assert.ok(observed.names.includes('Mikufans._aarch64.app.tar.gz'));
  const names = observed.names.map((name) => name.replaceAll('2.20.2', version));
  assert.throws(() => verifyUploadedReleaseAssets(actual(names), version, sha), /Release asset identity mismatch/);
});

test('a unique wrong product, a collision, missing signature and wrong build identity fail closed', () => {
  const names = releaseAssetNames(version).all;
  const wrong = [...names];
  const valid = releaseAssetNames(version).updater['darwin-aarch64'];
  wrong.splice(wrong.indexOf(valid), 1, `MikufansRecorder.WRONG_${version}_darwin_aarch64.app.tar.gz`);
  assert.throws(() => verifyUploadedReleaseAssets(actual(wrong), version, sha), /identity mismatch/);
  assert.throws(() => verifyUploadedReleaseAssets(actual([...names, names[0]]), version, sha), /colliding/);
  assert.throws(() => verifyUploadedReleaseAssets(actual(names.filter((name) => name !== `${valid}.sig`)), version, sha), /identity mismatch/);
  assert.throws(() => verifyUploadedReleaseAssets({ ...actual(names), targetCommitish: 'b'.repeat(40) }, version, sha), /approved commit/);
  assert.throws(() => verifyUploadedReleaseAssets({ ...actual(names), isDraft: false }, version, sha), /unpublished state/);
  assert.throws(() => verifyUploadedReleaseAssets({ ...actual(names), assets: [{ name: names[0], size: 0 }] }, version, sha), /identity mismatch/);
});
