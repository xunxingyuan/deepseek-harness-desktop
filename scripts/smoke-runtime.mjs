import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { once } from 'node:events'
import { request } from 'node:http'
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import * as tar from 'tar'

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const resources = join(projectRoot, 'src-tauri/runtime')
const manifest = JSON.parse(await readFile(join(resources, 'runtime-manifest.json'), 'utf8'))
const dependency = JSON.parse(await readFile(join(projectRoot, 'runtime/package.json'), 'utf8'))
assert.equal(manifest.harness, dependency.dependencies['@deepseek-ai/dsh'])
const node = join(resources, `dsh-runtime-${manifest.target}${process.platform === 'win32' ? '.exe' : ''}`)
const scratch = await mkdtemp(join(tmpdir(), 'dsh-desktop-smoke-'))
const runtime = join(scratch, 'runtime')
const home = join(scratch, 'harness/rc8')
const workspaceId = 'desktop-upgrade-fixture'
const sessionId = 'desktop-history-fixture'
const compression = process.argv.includes('--uncompressed') ? 'none' : 'zstd'
const redact = (value) => String(value).replace(/([?&]token=)[^\s]+/g, '$1<redacted>')

async function start() {
  const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) =>
    !/(?:KEY|SECRET|TOKEN|PASSWORD)/i.test(name) && !name.startsWith('DSH_') && name !== 'NODE_OPTIONS'))
  const child = spawn(node, [join(runtime, 'node_modules/@deepseek-ai/dsh/lib/bin.js'),
    'web', '--no-open', '--host', '127.0.0.1', '--port', '0', '--trusted-host', 'localhost'], {
    cwd: scratch,
    env: { ...environment, DSH_HOME: home, DSH_AGENTS_HOME: join(scratch, 'agents'),
      DSH_TELEMETRY_DISABLED: '1' },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  let output = ''
  const ready = new Promise((resolveReady, reject) => {
    const timer = setTimeout(() => reject(new Error(`Harness startup timed out: ${redact(output)}`)), 120_000)
    const append = (chunk) => {
      output = (output + chunk.toString()).slice(-80_000)
      const url = /dsh web: (http:\/\/127\.0\.0\.1:\d+\/\?token=[A-Za-z0-9_-]+)/.exec(output)?.[1]
      if (url) { clearTimeout(timer); resolveReady(new URL(url)) }
    }
    child.stdout.on('data', append)
    child.stderr.on('data', append)
    child.once('error', (error) => { clearTimeout(timer); reject(error) })
    child.once('exit', (code) => {
      clearTimeout(timer)
      reject(new Error(`Harness exited (${code}): ${redact(output)}`))
    })
  })
  const stop = async () => {
    if (child.exitCode !== null || child.signalCode !== null) return
    const exited = once(child, 'exit')
    child.kill('SIGTERM')
    const forced = setTimeout(() => child.kill('SIGKILL'), 10_000)
    try { await exited } finally { clearTimeout(forced) }
  }
  try { return { url: await ready, stop, diagnostics: () => redact(output) } }
  catch (error) { await stop(); throw error }
}

function http(url, route, { cookie, host = `localhost:${url.port}`, origin, body } = {}) {
  return new Promise((resolveResponse, reject) => {
    const data = body === undefined ? undefined : JSON.stringify(body)
    const req = request({ hostname: '127.0.0.1', port: url.port, path: route,
      method: data === undefined ? 'GET' : 'POST',
      headers: { host, ...(cookie ? { cookie } : {}), ...(origin ? { origin } : {}),
        ...(data ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(data) } : {}) },
    }, (res) => {
      let responseBody = ''
      res.setEncoding('utf8')
      res.on('data', (chunk) => { responseBody += chunk })
      res.on('end', () => resolveResponse({ status: res.statusCode, headers: res.headers, body: responseBody }))
    })
    req.on('error', reject)
    req.setTimeout(30_000, () => req.destroy(new Error('Runtime HTTP request timed out')))
    req.end(data)
  })
}

async function rpc(url, cookie, method, args) {
  const response = await http(url, `/api/${method}`, { cookie, origin: `http://localhost:${url.port}`,
    body: { type: 'client-request', rpcId: 'desktop-smoke', method, payload: { args } } })
  assert.equal(response.status, 200, `${method}: HTTP ${response.status}`)
  const envelope = JSON.parse(response.body)
  assert.equal(envelope.result?.ok, true, `${method}: ${JSON.stringify(envelope.result?.error)}`)
  return envelope.result.value
}

try {
  await mkdir(runtime, { recursive: true })
  await tar.x({ cwd: runtime, file: join(resources, 'dsh-runtime.tar.gz') })
  const entry = join(runtime, 'node_modules/@deepseek-ai/dsh/lib/bin.js')
  const version = spawnSync(node, [entry, '--version'], { encoding: 'utf8' })
  assert.equal(version.status, 0, redact(version.stderr))
  assert.equal(version.stdout.trim(), manifest.harness)

  await mkdir(join(scratch, 'existing-project'), { recursive: true })
  const workspacePath = await realpath(join(scratch, 'existing-project'))
  const projectKey = `--${workspacePath.replace(/[\\/:]+/g, '-').replace(/^-+/, '')}--`
  const sessionDir = join(home, 'sessions', projectKey, sessionId)
  await mkdir(sessionDir, { recursive: true })
  const legacyHeader = `${JSON.stringify({ type: 'session', version: 0, id: sessionId,
    cwd: workspacePath, createdAt: 1758585600000, delegationDepth: 0 })}\n`
  const compressed = compression === 'zstd' ? spawnSync(node, ['--input-type=module', '-e',
    "import { readFileSync } from 'node:fs'; import { zstdCompressSync } from 'node:zlib'; process.stdout.write(zstdCompressSync(readFileSync(0)));"],
  { input: legacyHeader }) : undefined
  if (compressed) assert.equal(compressed.status, 0, compressed.stderr.toString())
  const legacyFile = join(sessionDir, `session.jsonl${compression === 'zstd' ? '.zstd' : ''}`)
  const legacyBytes = compressed?.stdout ?? Buffer.from(legacyHeader)
  await writeFile(legacyFile, legacyBytes)
  await mkdir(join(home, 'storages'), { recursive: true })
  await mkdir(join(home, 'profiles/web'), { recursive: true })
  await writeFile(join(home, 'profiles/web/package.json'), JSON.stringify({
    dsh: { profile: { bundles: ['@deepseek-ai/dsh-base', '@deepseek-ai/dsh-web-app'] } },
  }))
  if (compression === 'none') {
    await writeFile(join(home, 'profiles/web/cordis.patch.yml'),
      "- id: session-persistence-jsonl\n  config:\n    root: !!js dshHomePath('sessions')\n    compression: none\n")
  }
  // Same workspace-v2 shape written by DSH Desktop 0.1.14. The new optional
  // fields are deliberately absent to exercise the upstream upgrade defaults.
  await writeFile(join(home, 'storages/workspace.json'), JSON.stringify({
    unit: { name: 'workspace', version: 2 },
    global: { initialized: true, workspaceIds: [workspaceId], archivedSessionIds: [] },
    tables: { workspaces: { [workspaceId]: { path: workspacePath, title: 'Existing project',
      sessionIds: [sessionId], createdAt: '2026-09-23T00:00:00.000Z', updatedAt: '2026-09-23T00:00:00.000Z' } } },
  }))

  let credentialsHash
  for (let pass = 0; pass < 2; pass++) {
    const running = await start()
    try {
      const { url } = running
      assert.equal((await http(url, '/')).status, 401, 'Unauthenticated index must be rejected')
      const exchange = await http(url, url.pathname + url.search)
      assert.equal(exchange.status, 303, 'WebView localhost token exchange')
      assert.equal(exchange.headers.location, './')
      const setCookie = exchange.headers['set-cookie']?.[0]
      assert.ok(setCookie?.includes('HttpOnly') && setCookie.includes('SameSite=Strict'))
      const cookie = setCookie.split(';', 1)[0]
      const index = await http(url, '/', { cookie })
      assert.equal(index.status, 200)
      assert.match(index.body, /<html/i)
      await rpc(url, cookie, 'settings/describe', {})
      const sessions = await rpc(url, cookie, 'session/list', { _request: {} })
      assert.ok(sessions.items.some((item) => item.sessionId === sessionId), 'Historical Session must be readable')
      await rpc(url, cookie, 'workspace/rename', { request: { workspaceId, title: 'Preserved project' } })
      assert.equal((await http(url, '/api/settings/describe', { cookie, host: 'untrusted.invalid',
        body: { type: 'client-request', rpcId: 'denied', method: 'settings/describe', payload: { args: {} } },
      })).status, 403, 'Untrusted Host must be rejected')
      const hash = createHash('sha256').update(await readFile(join(home, '.credentials.yaml'))).digest('hex')
      if (credentialsHash) assert.equal(hash, credentialsHash, 'Stored authentication must survive restart')
      credentialsHash = hash
    } catch (error) {
      throw new Error(`${error.message}\n${running.diagnostics()}`, { cause: error })
    } finally { await running.stop() }
    const storage = JSON.parse(await readFile(join(home, 'storages/workspace.json'), 'utf8'))
    assert.ok(storage.global.workspaceIds.includes(workspaceId))
    assert.equal(storage.tables.workspaces[workspaceId].title, 'Preserved project')
    assert.ok(storage.tables.workspaces[workspaceId].sessionIds.includes(sessionId))
    assert.deepEqual(await readFile(legacyFile), legacyBytes)
    console.log(`PASS ${pass === 0 ? 'upgrade startup' : 'restart'}: authentication, API, workspace v2, legacy Session, credentials`)
  }
  console.log(`Runtime smoke test passed: Harness ${manifest.harness} / ${manifest.target} / ${compression}`)
} finally {
  await rm(scratch, { recursive: true, force: true })
}
