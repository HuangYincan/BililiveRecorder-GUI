import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import upstream from '../upstream.json';
import './styles.css';

type BackendStatus = 'stopped' | 'starting' | 'running' | 'stopping' | 'failed';

interface LauncherState {
  status: BackendStatus;
  workdir: string | null;
  url: string | null;
  lastError: string | null;
  logs: string[];
}

interface UpdateInfo {
  version: string;
  body: string | null;
}

const workdirInput = document.querySelector<HTMLInputElement>('#workdir')!;
const statusText = document.querySelector<HTMLElement>('#status')!;
const errorBox = document.querySelector<HTMLElement>('#error')!;
const urlText = document.querySelector<HTMLElement>('#backend-url')!;
const logOutput = document.querySelector<HTMLElement>('#log-output')!;
const startButton = document.querySelector<HTMLButtonElement>('#start')!;
const openButton = document.querySelector<HTMLButtonElement>('#open')!;
const stopButton = document.querySelector<HTMLButtonElement>('#stop')!;
const chooseButton = document.querySelector<HTMLButtonElement>('#choose')!;
const updatePanel = document.querySelector<HTMLElement>('#update-panel')!;
const updateVersion = document.querySelector<HTMLElement>('#update-version')!;
const updateNotes = document.querySelector<HTMLElement>('#update-notes')!;
const installUpdateButton = document.querySelector<HTMLButtonElement>('#install-update')!;

document.querySelector('#backend-version')!.textContent = upstream.version;

const labels: Record<BackendStatus, string> = {
  stopped: '未启动',
  starting: '启动中',
  running: '运行中',
  stopping: '停止中',
  failed: '启动失败',
};

function setBusy(busy: boolean) {
  startButton.disabled = busy;
  chooseButton.disabled = busy;
  workdirInput.disabled = busy;
}

function render(state: LauncherState) {
  statusText.textContent = labels[state.status];
  statusText.dataset.status = state.status;
  if (state.workdir && document.activeElement !== workdirInput) workdirInput.value = state.workdir;
  urlText.textContent = state.url ?? '尚未启动';
  errorBox.hidden = !state.lastError;
  errorBox.textContent = state.lastError ?? '';
  logOutput.textContent = state.logs.length ? state.logs.join('\n') : '暂无日志';

  const transitional = state.status === 'starting' || state.status === 'stopping';
  setBusy(transitional);
  startButton.disabled = transitional || state.status === 'running';
  openButton.disabled = state.status !== 'running';
  stopButton.disabled = state.status !== 'running';
}

async function refresh() {
  render(await invoke<LauncherState>('get_launcher_state'));
}

async function run(action: () => Promise<unknown>) {
  errorBox.hidden = true;
  setBusy(true);
  try {
    await action();
  } catch (error) {
    errorBox.hidden = false;
    errorBox.textContent = String(error);
  } finally {
    await refresh();
  }
}

document.querySelector('#refresh')!.addEventListener('click', refresh);

chooseButton.addEventListener('click', () => run(async () => {
  const selected = await invoke<string | null>('choose_workdir');
  if (selected) workdirInput.value = selected;
}));

startButton.addEventListener('click', () => run(async () => {
  const workdir = workdirInput.value.trim();
  if (!workdir) throw new Error('请先选择工作目录。');
  await invoke('start_backend', { workdir });
  await invoke('open_recorder_window');
}));

openButton.addEventListener('click', () => run(() => invoke('open_recorder_window')));
stopButton.addEventListener('click', () => run(() => invoke('stop_backend')));
installUpdateButton.addEventListener('click', async () => {
  installUpdateButton.disabled = true;
  installUpdateButton.textContent = '正在下载并安装…';
  try {
    await invoke('install_update');
  } catch (error) {
    installUpdateButton.disabled = false;
    installUpdateButton.textContent = '重试安装';
    errorBox.hidden = false;
    errorBox.textContent = String(error);
  }
});

async function bootstrap() {
  await listen<LauncherState>('backend-state', (event) => render(event.payload));
  await refresh();
  try {
    const update = await invoke<UpdateInfo | null>('check_for_update');
    if (update) {
      updateVersion.textContent = `v${update.version}`;
      if (update.body) updateNotes.textContent = update.body;
      updatePanel.hidden = false;
    }
  } catch (error) {
    console.info('Update check unavailable:', error);
  }
}

void bootstrap();
