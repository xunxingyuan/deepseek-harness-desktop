# DSH Desktop

[English](README.md) | [简体中文](README.zh-CN.md)

这是一个基于 Tauri 2 的非官方、自包含
[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 桌面发行版。

用户只需下载并打开普通的 Windows 或 macOS 安装包。Node.js、DeepSeek
Harness 及其 Web UI 均已内置，无需安装 Node.js，也不需要手动运行
`npx @deepseek-ai/dsh web`。

> 本项目与 DeepSeek 无隶属或官方合作关系，也未获得 DeepSeek 官方背书。
> DeepSeek 和 DeepSeek Harness 的商标及项目权利归其各自权利人所有。

## 下载

**当前版本：[DSH Desktop v0.1.7](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/tag/v0.1.7)**

| 平台 | 推荐下载 |
| --- | --- |
| Windows x64 | [EXE 安装包](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/download/v0.1.7/DSH.Desktop_0.1.7_x64-setup.exe) |
| Windows x64（企业或集中部署） | [MSI 安装包](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/download/v0.1.7/DSH.Desktop_0.1.7_x64_zh-CN.msi) |
| Apple 芯片 Mac | [DMG 安装包](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/download/v0.1.7/DSH.Desktop_0.1.7_aarch64.dmg) |
| Intel 芯片 Mac | [DMG 安装包](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/download/v0.1.7/DSH.Desktop_0.1.7_x64.dmg) |

以后发布的新版本可以统一从
[最新版下载页面](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/latest)
获取。

> v0.1.2 起，macOS 安装包使用 Apple Developer ID 签名并提交 Apple 公证。

> v0.1.5 起，应用会在启动时检查最新 GitHub Release。发现新版后可以直接下载、
> 验证、安装并重启。v0.1.4 及更早版本需要手动安装一次 v0.1.5，之后才能使用自动更新。

DSH Desktop 启动后会在随机的 `127.0.0.1` 本地端口运行 Harness 服务，
等待服务正式就绪后打开内置 Web UI。关闭桌面应用时，Harness 进程也会一并停止。

## 安装包包含的组件

为了让发行构建可重复，主要组件均使用固定版本：

| 组件 | 版本 |
| --- | --- |
| DeepSeek Harness | `0.1.1-rc.1` |
| Node.js | `24.19.0`（Krypton LTS） |
| Tauri JavaScript API | `2.11.1` |
| Tauri CLI | `2.11.4` |

> Harness rc.8 使用了不兼容的 SQLite 存储格式。DSH Desktop v0.1.4 会使用新的
> 数据目录启动，同时保留旧版本本地数据以便回退。v0.1.6 起，应用会自动把旧工作区
> 记录和历史会话导入 rc.8，并保留当前已有的 rc.8 数据；迁移前会备份工作区索引，
> 原始旧数据也不会删除。Harness 0.1.1-rc.1 延续相同的数据格式，因此升级时继续
> 使用现有目录，不会再次隔离工作区。

运行时准备脚本会直接从 `nodejs.org` 下载 Node.js，使用官方 SHA-256
校验和验证文件，并按照构建机器的原生目标平台部署由锁文件固定的 Harness npm
依赖树。Harness 会以压缩归档形式写入安装包，首次启动时解压到当前用户的应用数据目录。

## 本地开发

以下环境仅供项目贡献者开发使用，最终用户不需要安装：

- Node.js 24
- pnpm 10
- Rust stable 以及对应平台的 Tauri 构建依赖

```bash
pnpm install
pnpm runtime:prepare
pnpm dev
```

生成当前平台的本地安装包：

```bash
pnpm build:desktop
```

自动生成的运行时文件位于 `src-tauri/runtime/`，该目录已加入忽略规则，
不会提交到 Git 仓库。

## 发布 GitHub Release

1. 将仓库推送到 GitHub，并确保默认分支名为 `main`。
2. 同步更新 `package.json`、`src-tauri/Cargo.toml` 和
   `src-tauri/tauri.conf.json` 中的版本号。
3. 创建并推送相同版本的标签，例如：

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

GitHub Actions 会在官方托管的原生运行器上构建以下目标：

- `x86_64-pc-windows-msvc`
- `aarch64-apple-darwin`
- `x86_64-apple-darwin`

构建期间，工作流会创建一个 GitHub 草稿 Release。所有平台全部成功后会自动公开发布；
如果任一平台失败，Release 将保持草稿状态，避免发布不完整的安装包。

工作流还会为三个平台生成 `latest.json` 和带签名的更新包，并在公开 Release 前
检查更新清单是否包含完整的 Windows、Apple 芯片 Mac 和 Intel Mac 下载信息。

## 应用签名

未签名的安装包可以运行，但 Windows SmartScreen 和 macOS Gatekeeper
可能会向用户显示安全警告。面向公众的正式发行版建议进行代码签名。

如需对 macOS 安装包进行签名和公证，请配置以下仓库 Secrets：

- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`

配置完成后，再将仓库变量 `ENABLE_APPLE_SIGNING` 设置为 `true`。只有显式开启该
变量时，发布工作流才会把签名凭据传递给 Tauri；否则即使仓库中残留了不完整的
Apple Secrets，也会生成未签名安装包，不会阻断 DMG 构建。

Windows 可使用 Authenticode 证书或 Microsoft Trusted Signing。请按照
[Tauri Windows 签名指南](https://v2.tauri.app/distribute/sign/windows/)
配置签名命令。切勿将证书或密码提交到仓库。

应用内更新使用一套独立的 Tauri 更新签名密钥。每次正式发布都需要配置以下
Repository Secrets：

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

与私钥配套的公钥会公开写入 `tauri.conf.json`，这是正常且安全的。请离线备份
更新私钥和密码；如果丢失，未来版本将无法为已经安装的客户端继续提供更新。

## 更新 DeepSeek Harness

Harness 目前仍处于开发预览阶段，版本升级可能包含不兼容变更。建议按照以下流程升级：

1. 修改 `runtime/package.json` 中的精确版本号。
2. 同步修改 `src-tauri/src/lib.rs`、`src/main.ts` 和
   `scripts/prepare-runtime.mjs` 中的 `HARNESS_VERSION`。
3. 在 `runtime` 目录执行 `npm install --package-lock-only`，重新生成
   `runtime/package-lock.json`。
4. 分别在 Windows、Apple 芯片 Mac 和 Intel Mac 上完成桌面端冒烟测试。
5. 发布新的桌面壳版本，不要覆盖已经发布的安装包。

## 开源许可

本桌面壳使用 MIT License。DeepSeek Harness、Node.js、Tauri
以及安装包中的其他依赖继续适用其各自的许可证，详情参见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
