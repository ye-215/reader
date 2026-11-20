#!/usr/bin/env node
// Builds the Windows desktop packages:
//   frontend -> release binary -> NSIS installer + portable zip
//
// The frontend assets are embedded into the exe at compile time by
// `tauri::generate_context!` (frontendDist), so the portable folder is just the
// executable:
//   Reader.exe
//   data/          <- created on first launch, next to the exe
//
// Outputs (in desktop/dist/):
//   <productName>_<ver>_x64-setup.exe   NSIS installer
//   reader-portable-v<ver>-win-x64.zip
//
// The installer filename is derived from the resolved `productName`, which the
// Windows build overrides in tauri.windows.conf.json (see readProductName).
//
// NOTE: cargo release builds must be low-concurrency (`-j 4`) because Windows
// antivirus real-time scanning races cargo's parallel .rlib writes and causes
// random "invalid metadata" / link.exe failures. We set CARGO_BUILD_JOBS=4 so
// the `tauri build` step inherits it.

import { execFileSync, execSync } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const frontendDir = join(repoRoot, 'frontend')
const desktopDir = join(repoRoot, 'desktop')
const tauriDir = join(desktopDir, 'src-tauri')
const outRoot = join(desktopDir, 'dist')
const stageDir = join(outRoot, 'Reader')
// tauri build resolves beforeBuildCommand's relative path (../../frontend)
// against the CWD, so it must run from src-tauri/ (where ../../frontend is
// correct). Invoke the CLI by absolute path to avoid npx re-resolving.
const tauriCli = join(desktopDir, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')

function run(command, args, cwd = repoRoot, env = {}) {
  console.log(`\n> ${command} ${args.join(' ')}`)
  execFileSync(command, args, { cwd, stdio: 'inherit', env: { ...process.env, ...env } })
}

/// For commands that need a shell to resolve (npm is npm.cmd on Windows).
function runShell(command, cwd = repoRoot, env = {}) {
  console.log(`\n> ${command}`)
  execSync(command, { cwd, stdio: 'inherit', env: { ...process.env, ...env } })
}

function readVersion() {
  const cargoToml = readFileSync(join(repoRoot, 'Cargo.toml'), 'utf8')
  const match = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)
  if (!match) throw new Error('无法从 Cargo.toml 读取版本号')
  return match[1]
}

/// The NSIS installer is named `<productName>_<version>_x64-setup.exe`, so the
/// name has to be read from the config rather than hardcoded: the Windows build
/// overrides `productName` in tauri.windows.conf.json (it must stay unique in
/// the Windows uninstall registry, see desktop/src-tauri/windows-hooks.nsh).
function readProductName() {
  const readJson = (file) =>
    JSON.parse(readFileSync(join(tauriDir, file), 'utf8').replace(/^\uFEFF/, ''))
  const base = readJson('tauri.conf.json')
  const windowsConf = join(tauriDir, 'tauri.windows.conf.json')
  const override = existsSync(windowsConf) ? readJson('tauri.windows.conf.json') : {}
  const productName = override.productName ?? base.productName
  if (!productName) throw new Error('无法从 tauri.conf.json 读取 productName')
  return productName
}

const version = readVersion()
const productName = readProductName()
console.log(`打包 阅读 桌面版 v${version}（productName: ${productName}）`)

// ── 加速开关（用环境变量控制，不改代码即可切换）──
// READER_SKIP_FRONTEND=1  只改了 Rust 时跳过 npm install + vite build，
//                         直接复用已有的 frontend/dist（前端 dist 会被
//                         tauri 编译进 exe，没改前端就没必要重建）。
// READER_BUNDLE=1         仅出安装包；默认会连同便携 zip 一起打。
//                         只想快速拿到可运行 exe 时设 READER_NO_BUNDLE=1，
//                         tauri build 加 --no-bundle，跳过 NSIS 压缩。
// CARGO_BUILD_JOBS        覆盖 cargo 并行度。默认保持原来的 4（避免占满
//                         全部 CPU 让电脑卡顿）；机器空闲时可显式调高。
const skipFrontend = process.env.READER_SKIP_FRONTEND === '1'
const noBundle = process.env.READER_NO_BUNDLE === '1'
const cargoJobs = process.env.CARGO_BUILD_JOBS?.trim() || '4'

// 可选的「固定 WebView2 运行时」打包模式。
//
// 默认走 Tauri 的 downloadBootstrapper: 体积小, 但用户机器上的 WebView2 版本
// 由系统更新决定 —— 新旧版本行为差异(渲染、媒体解码)只能靠运气。
//
// 设置 READER_FIXED_WEBVIEW2=<解压后的固定运行时目录> 后, 安装包会内嵌该版本
// (约 +180MB), 所有用户都跑同一个内核, 版本漂移类问题从根上消失。它要求目录里
// 有 msedgewebview2.exe 等运行时文件; 因为代价太大, 默认**不**开启, 只在需要
// 排查"某台机器渲染不对"或发长期支持版时用。
const fixedWebview2 = process.env.READER_FIXED_WEBVIEW2?.trim()
let bundleConfig = {}
if (fixedWebview2) {
  const runtimePath = resolve(repoRoot, fixedWebview2)
  if (!existsSync(join(runtimePath, 'msedgewebview2.exe'))) {
    throw new Error(
      `READER_FIXED_WEBVIEW2 指向的目录里没有 msedgewebview2.exe: ${runtimePath}\n` +
      '请下载 WebView2 Fixed Version（.cab 解压后）再指向它。',
    )
  }
  bundleConfig = {
    windows: {
      webviewInstallMode: { type: 'fixedRuntime', path: runtimePath },
    },
  }
  console.log(`使用固定 WebView2 运行时: ${runtimePath}（安装包将增大约 180MB）`)
}


// 用 `npm install` 而非 `npm ci`：ci 会先删除整个 node_modules，在受保护的
// 环境下批量删除会被安全策略拦截；install 是增量的，依赖已装好时几乎瞬间完成。
// 如需严格锁定版本，可保留 lock 文件并由 npm install 自动遵循。
if (skipFrontend) {
  console.log('\n[加速] READER_SKIP_FRONTEND=1：跳过 npm install + vite build，复用现有 frontend/dist')
} else {
  runShell('npm install', frontendDir)
  // Build the frontend here (beforeBuildCommand is a no-op) so the relative
  // path in tauri.conf.json never has to resolve against tauri's CWD.
  runShell('npm run build', frontendDir)
}

// `tauri build` runs cargo release build + NSIS bundling. Run from src-tauri/
// so the relative frontendDist path resolves correctly.
//
// --no-bundle：只要可运行 exe 时用，省掉 NSIS 压缩（几十秒~一分钟）。注意
// `--no-bundle` 时下面的 NSIS 复制会跳过；便携 zip 仍从 target/release/exe 打。
// --config 只在选择了固定 WebView2 运行时时传: 它会把这份内联配置与
// tauri.windows.conf.json 深合并, 不传时行为与改动前完全一致。
const buildArgs = [tauriCli, 'build']
if (noBundle) buildArgs.push('--no-bundle')
if (Object.keys(bundleConfig).length) {
  buildArgs.push('--config', JSON.stringify({ bundle: bundleConfig }))
}
console.log(`\n[加速] CARGO_BUILD_JOBS=${cargoJobs}${noBundle ? '，--no-bundle（跳过 NSIS）' : ''}`)
run('node', buildArgs, tauriDir, { CARGO_BUILD_JOBS: cargoJobs })

// ── NSIS installer ──
// The workspace shares the repo-root target/ dir, so the bundle lands under
// <repoRoot>/target/release/bundle/nsis, not desktop/src-tauri/target.
const nsisDir = join(repoRoot, 'target', 'release', 'bundle', 'nsis')
const nsisName = `${productName}_${version}_x64-setup.exe`
const nsisSrc = join(nsisDir, nsisName)
const nsisOut = join(outRoot, nsisName)
if (noBundle) {
  console.log('\n[加速] --no-bundle：跳过 NSIS 安装包复制')
} else {
  if (!existsSync(nsisSrc)) {
    throw new Error(`未找到 NSIS 安装包: ${nsisSrc}`)
  }
  rmSync(nsisOut, { force: true })
  cpSync(nsisSrc, nsisOut)
  console.log(`\nNSIS 安装包: ${nsisOut}`)
}

// ── Portable zip ──
const exeSrc = join(repoRoot, 'target', 'release', 'reader-desktop.exe')
if (!existsSync(exeSrc)) {
  throw new Error(`未找到编译产物: ${exeSrc}`)
}

rmSync(stageDir, { recursive: true, force: true })
mkdirSync(stageDir, { recursive: true })
cpSync(exeSrc, join(stageDir, 'Reader.exe'))
writeFileSync(join(stageDir, '.reader-portable'), 'Reader portable package\n')
// data/ is created at first launch — never ship it in the zip.
rmSync(join(stageDir, 'data'), { recursive: true, force: true })

const zipPath = join(outRoot, `reader-portable-v${version}-win-x64.zip`)
rmSync(zipPath, { force: true })
run('tar.exe', ['-a', '-c', '-f', zipPath, '-C', stageDir, '.'])

console.log(`\n完成:\n  安装包 ${noBundle ? '(本次 --no-bundle 已跳过)' : nsisOut}\n  压缩包 ${zipPath}`)
