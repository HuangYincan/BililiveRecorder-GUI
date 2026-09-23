import { readFileSync } from 'node:fs';
import { nextDesktopVersion, writeDesktopVersion } from './desktop-version.mjs';

// Run on an approved shell-fix branch before merging it; a tooling-only PR
// must not publish the existing (possibly still unsafe) lifecycle build.
const current = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).version;
const next = nextDesktopVersion(current, current);
writeDesktopVersion(next);
console.log(`Desktop shell ${current} -> ${next} (upstream CLI unchanged)`);
