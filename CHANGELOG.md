# Changelog

## 0.1.15 — 2026-10-19

适配 [DeepSeek Harness 0.2.1-rc.2](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.2.1-rc.2)。

- 跟进官方 DeepSeek-V4.1-Flash-1017 支持。
- 跟进 DeepSeek Search 和 Fetch API，可在网页搜索设置中开启。
- 适配新版 profile，清理已废弃的 `patchReload` 字段，保留自定义组合包和插件依赖。
- 保持既有 `harness/rc8` 数据目录，继续使用 workspace v2。
- 保留 macOS/Windows WebView 的原生认证流程和 Windows 独立 `dsh-runtime.exe` 运行时。
- 增加实际打包运行时的自动检查，覆盖本地认证、API、旧工作区读取和重启。

Updates to [DeepSeek Harness 0.2.1-rc.2](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.2.1-rc.2).

- Adds upstream DeepSeek-V4.1-Flash-1017 and DeepSeek Search/Fetch API support.
- Removes the retired `patchReload` profile field while retaining custom bundles and dependencies.
- Keeps the existing `harness/rc8` home and workspace-v2 data.
- Retains native WebView authentication and the dedicated Windows runtime executable.
- Adds a packaged-runtime smoke test for authentication, APIs, existing workspaces, and restart.
