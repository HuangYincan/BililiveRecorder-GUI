// GitHub release download names are deliberately ASCII. The app's display
// productName, bundle ID, installer metadata and icon remain unchanged.
// Pinned tauri-action v0 commit 84b9d35b renders exactly these placeholders.
export const ASSET_NAME_PATTERN = 'MikufansRecorder_[version]_[platform]_[arch][setup][ext]';
const base = 'MikufansRecorder';

export function releaseAssetNames(version) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`Invalid shell version: ${version}`);
  const name = (suffix) => `${base}_${version}_${suffix}`;
  const updater = {
    'darwin-aarch64': name('darwin_aarch64.app.tar.gz'),
    'darwin-x86_64': name('darwin_x64.app.tar.gz'),
    'linux-x86_64': name('linux_amd64.AppImage'),
    'windows-x86_64': name('windows_x64-setup.exe'),
  };
  const signed = [
    ...Object.values(updater),
    name('linux_amd64.deb'),
    name('linux_x86_64.rpm'),
    name('windows_x64.msi'),
  ];
  const installers = [
    name('darwin_aarch64.dmg'), name('darwin_x64.dmg'),
    name('linux_amd64.deb'), name('linux_x86_64.rpm'), name('linux_amd64.AppImage'),
    name('windows_x64.msi'), name('windows_x64-setup.exe'),
  ];
  const all = ['latest.json', ...new Set([...installers, ...Object.values(updater)]), ...signed.map((asset) => `${asset}.sig`)];
  if (new Set(all).size !== all.length) throw new Error('Release asset names collide');
  return { updater, signed, installers, all: all.sort() };
}
