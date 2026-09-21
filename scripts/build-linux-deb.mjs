import { spawnSync } from 'node:child_process'
import { cp, mkdir, readdir, stat } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const scriptDir = dirname(fileURLToPath(import.meta.url))
const projectRoot = resolve(scriptDir, '..')
const buildDir = join(projectRoot, 'build')
const targetDir = join(buildDir, 'target')
const tmpDir = join(buildDir, 'tmp')

function hostTriple() {
  const detected = spawnSync('rustc', ['--print', 'host-tuple'], { encoding: 'utf8' })
  if (detected.status !== 0 || !detected.stdout.trim()) {
    throw new Error('Unable to determine Rust host target triple; install the Rust toolchain first.')
  }
  return detected.stdout.trim()
}

function run(command, args, env) {
  console.log(`\n$ ${command} ${args.join(' ')}`)
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    stdio: 'inherit',
    env: { ...process.env, ...env },
  })
  if (result.error) throw result.error
  if (result.status !== 0) {
    throw new Error(`\`${command} ${args.join(' ')}\` failed with status ${result.status}`)
  }
}

async function exists(path) {
  try {
    await stat(path)
    return true
  } catch {
    return false
  }
}

async function collectFiles(directory, predicate, accumulator = []) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      await collectFiles(path, predicate, accumulator)
    } else if (entry.isFile() && predicate(path)) {
      accumulator.push(path)
    }
  }
  return accumulator
}

async function main() {
  const target = hostTriple()
  if (!target.includes('-linux-')) {
    throw new Error(`build:linux targets Linux hosts only; detected ${target}.`)
  }

  // Keep every intermediate artifact inside ./build so the tree stays clean and
  // the final .deb is easy to locate.
  await mkdir(targetDir, { recursive: true })
  await mkdir(tmpDir, { recursive: true })
  const env = {
    CARGO_TARGET_DIR: targetDir,
    TMPDIR: tmpDir,
    TAURI_ENV_TARGET_TRIPLE: target,
  }

  if (!(await exists(join(projectRoot, 'node_modules')))) {
    run('pnpm', ['install', '--frozen-lockfile'], env)
  }

  // 1. Self-contained runtime: bundled Node.js + DeepSeek Harness tarball.
  run('pnpm', ['runtime:prepare', '--', '--target', target], env)
  await cp(join(projectRoot, 'src-tauri', 'runtime'), join(buildDir, 'runtime'), {
    recursive: true,
    force: true,
  })

  // 2. Frontend bundle + Tauri deb package. When no updater signing key is
  // present (typical local build), disable updater artifact generation so the
  // deb is produced without requiring TAURI_SIGNING_PRIVATE_KEY.
  const tauriArgs = ['tauri', 'build', '--bundles', 'deb', '--target', target]
  if (!process.env.TAURI_SIGNING_PRIVATE_KEY) {
    tauriArgs.push('--config', JSON.stringify({ bundle: { createUpdaterArtifacts: false } }))
    console.log('\nNo TAURI_SIGNING_PRIVATE_KEY set; building an unsigned deb without updater artifacts.')
  }
  run('pnpm', tauriArgs, env)

  if (await exists(join(projectRoot, 'dist'))) {
    await cp(join(projectRoot, 'dist'), join(buildDir, 'dist'), { recursive: true, force: true })
  }

  // 3. Surface the final package(s) at the top of ./build.
  const packages = await collectFiles(
    buildDir,
    (path) => path.endsWith('.deb') || path.endsWith('.deb.sig'),
  )
  const finalArtifacts = []
  for (const pkg of packages) {
    // Skip anything already sitting directly in buildDir.
    if (dirname(pkg) === buildDir) {
      finalArtifacts.push(pkg)
      continue
    }
    const destination = join(buildDir, pkg.split('/').pop())
    await cp(pkg, destination, { force: true })
    finalArtifacts.push(destination)
  }

  console.log('\nBuild finished. Final artifacts in ./build:')
  for (const artifact of finalArtifacts.sort()) {
    console.log(`  - ${artifact.slice(projectRoot.length + 1)}`)
  }
  console.log(`\nIntermediates: build/target (cargo), build/runtime, build/dist, build/tmp`)
}

await main()
