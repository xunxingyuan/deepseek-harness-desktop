import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import './styles.css'

export type BackendPhase = 'starting' | 'running' | 'failed' | 'stopped'

export interface BackendStatus {
  phase: BackendPhase
  message: string
  url: string | null
  harnessVersion: string
}

const root = document.querySelector<HTMLElement>('#app')

if (!root) {
  throw new Error('Missing application root')
}
const appRoot: HTMLElement = root

let retrying = false

function escapeHtml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#039;')
}

function render(status: BackendStatus): void {
  const failed = status.phase === 'failed' || status.phase === 'stopped'
  appRoot.innerHTML = `
    <section class="shell ${failed ? 'shell--failed' : ''}">
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
        <div class="status-block">
          <div class="spinner ${failed ? 'spinner--failed' : ''}" aria-hidden="true"></div>
          <div>
            <h2>${failed ? '启动遇到问题' : '正在准备工作空间'}</h2>
            <p>${escapeHtml(status.message)}</p>
          </div>
        </div>
        ${failed ? '<button id="retry" type="button">重新启动</button>' : ''}
        <footer>
          <span>Harness ${escapeHtml(status.harnessVersion)}</span>
          <span>Local-only · 127.0.0.1</span>
        </footer>
      </div>
    </section>
  `

  document.querySelector<HTMLButtonElement>('#retry')?.addEventListener('click', () => {
    void restart()
  })
}

function navigateToHarness(url: string): void {
  const parsed = new URL(url)
  if (parsed.protocol !== 'http:' || parsed.hostname !== '127.0.0.1') {
    render({
      phase: 'failed',
      message: '后台返回了不安全的地址，桌面壳已阻止跳转。',
      url: null,
      harnessVersion: '0.1.0-rc.8',
    })
    return
  }
  window.location.replace(parsed.toString())
}

function applyStatus(status: BackendStatus): void {
  if (status.phase === 'running' && status.url) {
    navigateToHarness(status.url)
    return
  }
  render(status)
}

async function restart(): Promise<void> {
  if (retrying) return
  retrying = true
  render({
    phase: 'starting',
    message: '正在重新启动内置 Harness…',
    url: null,
    harnessVersion: '0.1.0-rc.8',
  })
  try {
    applyStatus(await invoke<BackendStatus>('restart_backend'))
  } catch (error) {
    render({
      phase: 'failed',
      message: String(error),
      url: null,
      harnessVersion: '0.1.0-rc.8',
    })
  } finally {
    retrying = false
  }
}

async function bootstrap(): Promise<void> {
  render({
    phase: 'starting',
    message: '正在启动内置 Node.js 与 DeepSeek Harness…',
    url: null,
    harnessVersion: '0.1.0-rc.8',
  })

  await listen<BackendStatus>('backend-status', (event) => {
    applyStatus(event.payload)
  })

  try {
    applyStatus(await invoke<BackendStatus>('backend_status'))
  } catch (error) {
    render({
      phase: 'failed',
      message: String(error),
      url: null,
      harnessVersion: '0.1.0-rc.8',
    })
  }
}

void bootstrap()
