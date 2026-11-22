import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { createReadStream, createWriteStream } from 'node:fs'
import { copyFile, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { availableParallelism, tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath } from 'node:url'

const config = JSON.parse(await readFile(new URL('./git-runtime.json', import.meta.url), 'utf8'))
const isMac = process.platform === 'darwin'
const platform = isMac ? config.macos : config.windows
const resources = fileURLToPath(new URL('../src-tauri/resources/', import.meta.url))
const runtime = join(resources, 'git')
const expectedVersion = `git version ${config.version}${platform.versionSuffix ?? ''}`
const target = `${process.platform}-${process.arch}`

function run(executable, args, options = {}) {
  const result = spawnSync(executable, args, { stdio: 'inherit', ...options })
  if (result.error) throw result.error
  if (result.status !== 0) {
    throw new Error(`${executable} 退出异常（${result.status ?? result.signal}）`)
  }
  return result
}

function installedVersion(directory) {
  const result = spawnSync(join(directory, platform.executable), ['--version'], {
    encoding: 'utf8',
  })
  return result.status === 0 ? result.stdout.trim() : null
}

async function prepare() {
  if (
    !(isMac && ['arm64', 'x64'].includes(process.arch)) &&
    !(process.platform === 'win32' && process.arch === 'x64')
  ) {
    throw new Error(`一期随包 Git 支持 macOS arm64/x64 和 Windows x64，当前为 ${target}`)
  }

  const manifest = { target, version: config.version, source: platform.url, sha256: platform.sha256 }
  const manifestPath = join(runtime, 'filemanagement-runtime.json')
  let previous
  try {
    previous = JSON.parse(await readFile(manifestPath, 'utf8'))
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
  }
  if (
    previous?.target === manifest.target &&
    previous?.sha256 === manifest.sha256 &&
    installedVersion(runtime) === expectedVersion
  ) {
    console.log(`已准备 ${expectedVersion}（${target}）`)
    return
  }

  await mkdir(resources, { recursive: true })
  const work = await mkdtemp(join(tmpdir(), 'filemanagement-git-build-'))
  const staging = await mkdtemp(join(resources, '.git-stage-'))
  try {
    // 允许使用预先下载的同一个官方归档，校验规则始终一致。
    const archive = process.env.FILEMANAGEMENT_GIT_ARCHIVE
      ? resolve(process.env.FILEMANAGEMENT_GIT_ARCHIVE)
      : join(work, platform.archive)
    if (!process.env.FILEMANAGEMENT_GIT_ARCHIVE) {
      console.log(`下载 ${platform.archive}`)
      const response = await fetch(platform.url, { signal: AbortSignal.timeout(300_000) })
      if (!response.ok || !response.body) {
        throw new Error(`下载随包 Git 失败（HTTP ${response.status}）`)
      }
      await pipeline(Readable.fromWeb(response.body), createWriteStream(archive, { flags: 'wx' }))
    }

    const sha256 = createHash('sha256')
    for await (const chunk of createReadStream(archive)) sha256.update(chunk)
    if (sha256.digest('hex') !== platform.sha256) {
      throw new Error(`随包 Git 归档校验失败（${platform.archive}）`)
    }

    if (isMac) {
      run('tar', ['-xf', archive, '-C', work])
      const source = join(work, `git-${config.version}`)
      run('make', [
        `-j${Math.min(availableParallelism(), 8)}`,
        'NO_CURL=YesPlease', 'NO_EXPAT=YesPlease', 'NO_GETTEXT=YesPlease',
        'NO_OPENSSL=YesPlease', 'NO_TCLTK=YesPlease', 'NO_PERL=YesPlease',
        'NO_PYTHON=YesPlease', 'git',
      ], { cwd: source, env: { ...process.env, MACOSX_DEPLOYMENT_TARGET: '11.0' } })
      await mkdir(dirname(join(staging, platform.executable)), { recursive: true })
      await copyFile(join(source, 'git'), join(staging, platform.executable))
      await copyFile(join(source, 'COPYING'), join(staging, 'COPYING'))
    } else {
      run('tar.exe', ['-xf', archive, '-C', staging])
    }

    const version = installedVersion(staging)
    if (version !== expectedVersion) {
      throw new Error(`随包 Git 版本不符，预期 ${expectedVersion}，实际 ${version ?? '无法启动'}`)
    }

    await writeFile(join(staging, 'filemanagement-runtime.json'), JSON.stringify(manifest, null, 2))
    if (previous?.source && previous?.sha256) await rm(runtime, { recursive: true, force: true })
    await rename(staging, runtime)
    console.log(`已准备 ${expectedVersion}（${target}）`)
  } finally {
    await rm(staging, { recursive: true, force: true })
    await rm(work, { recursive: true, force: true })
  }
}

await prepare()
