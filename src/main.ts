import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { relaunch } from '@tauri-apps/plugin-process'
import { check, type Update } from '@tauri-apps/plugin-updater'
import {
  emptyDownloadProgress,
  formatBytes,
  updateDownloadProgress,
  type DownloadProgress,
} from './update-progress'
import './styles.css'

export type BackendPhase = 'starting' | 'running' | 'failed' | 'stopped'

export interface BackendStatus {
  phase: BackendPhase
  message: string
  url: string | null
  harnessVersion: string
}

const APP_VERSION = '0.1.5'
const HARNESS_VERSION = '0.1.0-rc.8'
const UPDATE_CHECK_TIMEOUT_MS = 5_000

const root = document.querySelector<HTMLElement>('#app')

if (!root) {
  throw new Error('Missing application root')
}
const appRoot: HTMLElement = root

let retrying = false
let navigating = false
let updateCheckFinished = false
let pendingUpdate: Update | null = null
let installingUpdate = false
let installStage = ''
let installError = ''
let downloadProgress: DownloadProgress = emptyDownloadProgress
let latestBackendStatus: BackendStatus = {
  phase: 'starting',
  message: '正在启动内置 DeepSeek Harness…',
  url: null,
  harnessVersion: HARNESS_VERSION,
}

function escapeHtml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#039;')
}

function frame(content: string): string {
  return `
    <section class="shell">
      <div class="glow glow--top"></div>
      <div class="glow glow--bottom"></div>
      <div class="card">
        <div class="brand" aria-label="DSH Desktop">
          <div class="mark"><span></span><span></span><span></span></div>
          <div>
            <p class="eyebrow">DESKTOP RUNTIME</p>
            <h1>DSH Desktop</h1>
          </div>
        </div>
        ${content}
        <footer>
          <span>DSH Desktop ${APP_VERSION}</span>
          <span>Harness ${HARNESS_VERSION} · Local-only</span>
        </footer>
      </div>
    </section>
  `
}

function renderStatus(status: BackendStatus, checkingUpdate = false): void {
  const failed = status.phase === 'failed' || status.phase === 'stopped'
  const title = failed
    ? '启动遇到问题'
    : checkingUpdate
      ? '正在检查更新'
      : '正在准备工作空间'
  const message = checkingUpdate
    ? '正在连接 GitHub 检查新版本，同时启动内置 Harness…'
    : status.message

  appRoot.innerHTML = frame(`
    <div class="status-block ${failed ? 'status-block--failed' : ''}">
      <div class="spinner ${failed ? 'spinner--failed' : ''}" aria-hidden="true"></div>
      <div>
        <h2>${title}</h2>
        <p>${escapeHtml(message)}</p>
      </div>
    </div>
    ${failed ? '<button id="retry" type="button">重新启动</button>' : ''}
  `)

  document.querySelector<HTMLButtonElement>('#retry')?.addEventListener('click', () => {
    void restart()
  })
}

function progressDescription(progress: DownloadProgress): string {
  if (installStage === 'restarting') return '更新安装完成，正在重新启动…'
  if (progress.finished) return '下载完成，正在安装并准备重启…'
  if (progress.percent !== undefined && progress.contentLength !== undefined) {
    return `${progress.percent}% · ${formatBytes(progress.downloadedBytes)} / ${formatBytes(progress.contentLength)}`
  }
  if (progress.downloadedBytes > 0) {
    return `已下载 ${formatBytes(progress.downloadedBytes)}`
  }
  return '正在连接下载服务器…'
}

function renderUpdate(): void {
  const update = pendingUpdate
  if (!update) return

  const releaseNotes = update.body?.trim() || '包含功能改进和问题修复。'
  const percent = downloadProgress.percent ?? 0
  const progressClass = downloadProgress.percent === undefined ? 'progress-bar--indeterminate' : ''

  appRoot.innerHTML = frame(`
    <div class="update-block" aria-live="polite">
      <div class="update-title">
        <span class="update-badge">NEW</span>
        <h2>发现新版本 ${escapeHtml(update.version)}</h2>
      </div>
      <p class="update-summary">当前版本 ${escapeHtml(update.currentVersion)}，可以直接在应用内完成安全更新。</p>
      <div class="release-notes" aria-label="更新说明">${escapeHtml(releaseNotes)}</div>
      ${installingUpdate ? `
        <div class="progress" aria-label="下载进度">
          <div class="progress-track"><span class="${progressClass}" style="width: ${percent}%"></span></div>
          <p>${escapeHtml(progressDescription(downloadProgress))}</p>
        </div>
      ` : ''}
      ${installError ? `<p class="update-error">更新失败：${escapeHtml(installError)}。你可以重试，或稍后继续使用当前版本。</p>` : ''}
      <div class="update-actions">
        <button id="skip-update" class="button-secondary" type="button" ${installingUpdate ? 'disabled' : ''}>稍后更新</button>
        <button id="install-update" type="button" ${installingUpdate ? 'disabled' : ''}>${installingUpdate ? '正在更新…' : installError ? '重新尝试' : '立即更新'}</button>
      </div>
    </div>
  `)

  document.querySelector<HTMLButtonElement>('#skip-update')?.addEventListener('click', () => {
    void skipUpdate()
  })
  document.querySelector<HTMLButtonElement>('#install-update')?.addEventListener('click', () => {
    void installPendingUpdate()
  })
}

function navigateToHarness(url: string): void {
  if (navigating) return
  const parsed = new URL(url)
  if (parsed.protocol !== 'http:' || parsed.hostname !== '127.0.0.1') {
    latestBackendStatus = {
      phase: 'failed',
      message: '后台返回了不安全的地址，桌面壳已阻止跳转。',
      url: null,
      harnessVersion: HARNESS_VERSION,
    }
    renderStatus(latestBackendStatus)
    return
  }
  navigating = true
  window.location.replace(parsed.toString())
}

function present(): void {
  if (pendingUpdate) {
    renderUpdate()
    return
  }

  const failed = latestBackendStatus.phase === 'failed' || latestBackendStatus.phase === 'stopped'
  if (!updateCheckFinished && !failed) {
    renderStatus(latestBackendStatus, true)
    return
  }

  if (latestBackendStatus.phase === 'running' && latestBackendStatus.url) {
    navigateToHarness(latestBackendStatus.url)
    return
  }
  renderStatus(latestBackendStatus)
}

async function checkForUpdates(): Promise<void> {
  try {
    pendingUpdate = await check({ timeout: UPDATE_CHECK_TIMEOUT_MS })
  } catch (error) {
    console.warn('Unable to check for DSH Desktop updates', error)
  } finally {
    updateCheckFinished = true
    present()
  }
}

async function skipUpdate(): Promise<void> {
  const update = pendingUpdate
  pendingUpdate = null
  installError = ''
  if (update) {
    try {
      await update.close()
    } catch (error) {
      console.warn('Unable to close the skipped update resource', error)
    }
  }
  present()
}

async function installPendingUpdate(): Promise<void> {
  const update = pendingUpdate
  if (!update || installingUpdate) return

  installingUpdate = true
  installStage = 'downloading'
  installError = ''
  downloadProgress = emptyDownloadProgress
  renderUpdate()

  try {
    await update.downloadAndInstall((event) => {
      downloadProgress = updateDownloadProgress(downloadProgress, event)
      if (event.event === 'Finished') installStage = 'installing'
      renderUpdate()
    })
    installStage = 'restarting'
    renderUpdate()
    await relaunch()
  } catch (error) {
    installingUpdate = false
    installStage = ''
    installError = String(error)
    renderUpdate()
  }
}

async function restart(): Promise<void> {
  if (retrying) return
  retrying = true
  latestBackendStatus = {
    phase: 'starting',
    message: '正在重新启动内置 Harness…',
    url: null,
    harnessVersion: HARNESS_VERSION,
  }
  present()
  try {
    latestBackendStatus = await invoke<BackendStatus>('restart_backend')
    present()
  } catch (error) {
    latestBackendStatus = {
      phase: 'failed',
      message: String(error),
      url: null,
      harnessVersion: HARNESS_VERSION,
    }
    present()
  } finally {
    retrying = false
  }
}

async function bootstrap(): Promise<void> {
  renderStatus(latestBackendStatus, true)

  await listen<BackendStatus>('backend-status', (event) => {
    latestBackendStatus = event.payload
    present()
  })

  void checkForUpdates()

  try {
    latestBackendStatus = await invoke<BackendStatus>('backend_status')
  } catch (error) {
    latestBackendStatus = {
      phase: 'failed',
      message: String(error),
      url: null,
      harnessVersion: HARNESS_VERSION,
    }
  }
  present()
}

void bootstrap()
