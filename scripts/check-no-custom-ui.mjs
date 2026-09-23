import { existsSync, readFileSync } from 'node:fs';

const forbidden = ['index.html', 'src/main.ts', 'src/styles.css', 'vite.config.ts'];
const present = forbidden.filter((path) => existsSync(path));
if (present.length) throw new Error(`Custom frontend files must not exist: ${present.join(', ')}`);

const config = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
if (config.app.windows.length !== 0) throw new Error('Tauri must not create a local frontend window.');
console.log('No custom GUI is bundled; the runtime creates only the upstream WebUI window.');
