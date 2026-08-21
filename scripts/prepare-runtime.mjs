import { createHash } from 'node:crypto'
import { chmod, copyFile, cp, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import extractZip from 'extract-zip'
import * as tar from 'tar'

const NODE_VERSION = 'v24.19.0'
const HARNESS_VERSION = '0.1.1-rc.1'
const scriptDir = dirname(fileURLToPath(import.meta.url))
const projectRoot = resolve(scriptDir, '..')
const runtimeDir = join(projectRoot, 'src-tauri', 'runtime')
const runtimePackageDir = join(projectRoot, 'runtime')

function argument(name) {
  const index = process.argv.indexOf(name)
  return index === -1 ? undefined : process.argv[index + 1]
}

function hostTriple() {
  const explicit = argument('--target') || process.env.TAURI_ENV_TARGET_TRIPLE
  if (explicit) return explicit
  const detected = spawnSync('rustc', ['--print', 'host-tuple'], { encoding: 'utf8' })
  if (detected.status !== 0 || !detected.stdout.trim()) {
    throw new Error('Unable to determine Rust host target; pass --target <triple>.')
  }
  return detected.stdout.trim()
}

function nodeDistribution(target) {
  const table = {
    'aarch64-apple-darwin': { platform: 'darwin', arch: 'arm64', extension: 'tar.gz', binary: 'bin/node' },
    'x86_64-apple-darwin': { platform: 'darwin', arch: 'x64', extension: 'tar.gz', binary: 'bin/node' },
    'aarch64-pc-windows-msvc': { platform: 'win', arch: 'arm64', extension: 'zip', binary: 'node.exe' },
    'x86_64-pc-windows-msvc': { platform: 'win', arch: 'x64', extension: 'zip', binary: 'node.exe' },
  }
  const distribution = table[target]
  if (!distribution) {
    throw new Error(`Unsupported release target: ${target}`)
  }
  return distribution
}

async function download(url, destination) {
  const response = await fetch(url, { redirect: 'follow' })
  if (!response.ok) throw new Error(`Download failed (${response.status}): ${url}`)
  await writeFile(destination, Buffer.from(await response.arrayBuffer()))
}

async function verifyArchive(archive, filename, checksumsPath) {
  const checksums = await readFile(checksumsPath, 'utf8')
  const expected = checksums
    .split(/\r?\n/)
    .find((line) => line.endsWith(`  ${filename}`))
    ?.split(/\s+/)[0]
  if (!expected) throw new Error(`No checksum found for ${filename}`)
  const actual = createHash('sha256').update(await readFile(archive)).digest('hex')
  if (actual !== expected) throw new Error(`Checksum mismatch for ${filename}`)
}

async function* filesWithin(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      yield* filesWithin(path)
    } else if (entry.isFile()) {
      yield path
    }
  }
}

async function signEmbeddedMacBinaries(directory, target) {
  if (!target.includes('apple-darwin')) return

  const identity = process.env.APPLE_SIGNING_IDENTITY?.trim()
  if (!identity) return

  let signed = 0
  for await (const path of filesWithin(directory)) {
    const type = spawnSync('file', ['-b', path], { encoding: 'utf8' })
    if (type.error) throw new Error(`Unable to inspect ${path}: ${type.error.message}`)
    if (type.status !== 0) throw new Error(`file failed for ${path} with status ${type.status}`)
    if (!type.stdout.includes('Mach-O')) continue

    console.log(`Signing embedded native binary: ${path}`)
    const sign = spawnSync('codesign', [
      '--force',
      '--options', 'runtime',
      '--timestamp',
      '--sign', identity,
      path,
    ], { stdio: 'inherit' })
    if (sign.error) throw new Error(`Unable to codesign ${path}: ${sign.error.message}`)
    if (sign.status !== 0) throw new Error(`codesign failed for ${path} with status ${sign.status}`)
    signed += 1
  }

  console.log(`Signed ${signed} embedded macOS native binaries with secure timestamps.`)
}

async function main() {
  if (basename(runtimeDir) !== 'runtime' || basename(dirname(runtimeDir)) !== 'src-tauri') {
    throw new Error(`Refusing to prepare unexpected runtime directory: ${runtimeDir}`)
  }

  const target = hostTriple()
  const distribution = nodeDistribution(target)
  const stem = `node-${NODE_VERSION}-${distribution.platform}-${distribution.arch}`
  const filename = `${stem}.${distribution.extension}`
  const baseUrl = `https://nodejs.org/dist/${NODE_VERSION}`
  const temporary = await mkdtemp(join(tmpdir(), 'dsh-desktop-runtime-'))

  try {
    const archive = join(temporary, filename)
    const checksums = join(temporary, 'SHASUMS256.txt')
    console.log(`Preparing Node.js ${NODE_VERSION} and DeepSeek Harness ${HARNESS_VERSION} for ${target}`)
    await Promise.all([
      download(`${baseUrl}/${filename}`, archive),
      download(`${baseUrl}/SHASUMS256.txt`, checksums),
    ])
    await verifyArchive(archive, filename, checksums)

    if (distribution.extension === 'zip') {
      await extractZip(archive, { dir: temporary })
    } else {
      await tar.x({ file: archive, cwd: temporary })
    }

    await rm(runtimeDir, { recursive: true, force: true })
    const dshDir = join(runtimeDir, 'dsh')
    await mkdir(dshDir, { recursive: true })

    const extracted = join(temporary, stem)
    const binarySuffix = target.includes('windows') ? '.exe' : ''
    const sidecar = join(runtimeDir, `node-${target}${binarySuffix}`)
    await copyFile(join(extracted, distribution.binary), sidecar)
    if (!binarySuffix) await chmod(sidecar, 0o755)

    await cp(join(runtimePackageDir, 'package.json'), join(dshDir, 'package.json'))
    await cp(join(runtimePackageDir, 'package-lock.json'), join(dshDir, 'package-lock.json'))
    const npmArgs = ['ci', '--omit=dev', '--no-audit', '--no-fund']
    const npmCommand = process.platform === 'win32'
      ? { executable: process.env.ComSpec || 'cmd.exe', args: ['/d', '/s', '/c', 'npm.cmd', ...npmArgs] }
      : { executable: 'npm', args: npmArgs }
    const install = spawnSync(npmCommand.executable, npmCommand.args, {
      cwd: dshDir,
      stdio: 'inherit',
      env: {
        ...process.env,
        npm_config_cache: join(projectRoot, '.cache', 'npm'),
      },
    })
    if (install.error) throw new Error(`Unable to start npm ci: ${install.error.message}`)
    if (install.status !== 0) throw new Error(`npm ci failed with status ${install.status}`)

    await copyFile(join(extracted, 'LICENSE'), join(dshDir, 'NODE_LICENSE'))
    const entry = join(dshDir, 'node_modules', '@deepseek-ai', 'dsh', 'lib', 'bin.js')
    await readFile(entry)
    await signEmbeddedMacBinaries(dshDir, target)
    const manifest = `${JSON.stringify({
      node: NODE_VERSION,
      harness: HARNESS_VERSION,
      target,
    }, null, 2)}\n`
    await writeFile(join(runtimeDir, 'runtime-manifest.json'), manifest)
    await tar.c({
      cwd: dshDir,
      file: join(runtimeDir, 'dsh-runtime.tar.gz'),
      gzip: true,
      portable: true,
    }, ['.'])
    await rm(dshDir, { recursive: true, force: true })
    console.log(`Runtime ready: ${sidecar}`)
  } finally {
    await rm(temporary, { recursive: true, force: true })
  }
}

await main()
