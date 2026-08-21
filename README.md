# DSH Desktop

[English](README.md) | [简体中文](README.zh-CN.md)

An unofficial, self-contained Tauri 2 desktop distribution of
[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness).

Users download and open a normal Windows or macOS installer. Node.js,
DeepSeek Harness, and its Web UI are already bundled: there is no need to
install Node.js or run `npx @deepseek-ai/dsh web`.

> This project is not affiliated with or endorsed by DeepSeek. DeepSeek and
> DeepSeek Harness are trademarks or projects of their respective owners.

## Download

**Current release: [DSH Desktop v0.1.6](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/tag/v0.1.6)**

| Platform | Recommended download |
| --- | --- |
| Windows x64 | [EXE installer](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/download/v0.1.6/DSH.Desktop_0.1.6_x64-setup.exe) |
| Windows x64 (managed deployment) | [MSI installer](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/download/v0.1.6/DSH.Desktop_0.1.6_x64_zh-CN.msi) |
| Apple Silicon Mac | [DMG installer](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/download/v0.1.6/DSH.Desktop_0.1.6_aarch64.dmg) |
| Intel Mac | [DMG installer](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/download/v0.1.6/DSH.Desktop_0.1.6_x64.dmg) |

You can always find the newest version on the
[Latest Release](https://github.com/xunxingyuan/deepseek-harness-desktop/releases/latest)
page.

> Starting with v0.1.2, macOS installers are signed with Apple Developer ID and
> submitted to Apple for notarization.

> Starting with v0.1.5, DSH Desktop checks the latest GitHub Release during
> startup and can download, verify, install, and restart into a newer version.
> Users of v0.1.4 and earlier must install v0.1.5 manually once before automatic
> updates become available.

DSH Desktop starts a private Harness server on a random `127.0.0.1` port, waits for
its official readiness signal, and opens the built-in Web UI. Closing the app
also stops the Harness process.

## What gets bundled

Versions are deliberately pinned for reproducible releases:

| Component | Version |
| --- | --- |
| DeepSeek Harness | `0.1.0-rc.8` |
| Node.js | `24.19.0` (Krypton LTS) |
| Tauri JavaScript API | `2.11.1` |
| Tauri CLI | `2.11.4` |

> Harness rc.8 uses an incompatible SQLite storage format. DSH Desktop v0.1.4
> starts it in a new data directory and keeps older local data untouched for
> rollback. Starting with v0.1.6, DSH Desktop automatically imports legacy
> workspace records and session history into rc.8 without overwriting current
> rc.8 data. It backs up the current workspace index before migration and keeps
> the original legacy data intact.

The runtime preparation step downloads Node.js directly from `nodejs.org`,
verifies its official SHA-256 checksum, and deploys the locked Harness npm
dependency tree for the build machine's native target. Harness is stored as a
compressed, symlink-preserving archive in the installer and expanded once into
the per-user app-data directory on first launch.

## Local development

Requirements for contributors only:

- Node.js 24
- pnpm 10
- Rust stable and the normal Tauri platform prerequisites

```bash
pnpm install
pnpm runtime:prepare
pnpm dev
```

Create a local installer with:

```bash
pnpm build:desktop
```

Generated runtime files live in `src-tauri/runtime/` and are intentionally not
committed.

## Publishing a GitHub Release

1. Push this repository to GitHub with the default branch named `main`.
2. Update the version in `package.json`, `src-tauri/Cargo.toml`, and
   `src-tauri/tauri.conf.json`.
3. Commit and push a matching tag, for example:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

The release workflow builds these targets on native GitHub-hosted runners:

- `x86_64-pc-windows-msvc`
- `aarch64-apple-darwin`
- `x86_64-apple-darwin`

It creates a draft GitHub Release while installers are building, then publishes
the release automatically after every platform succeeds. A failed build leaves
the release as a draft so incomplete artifacts are not published.

The workflow also generates `latest.json` plus signed updater artifacts for all
three targets. It validates the update manifest before making the Release public.

## Signing

Unsigned builds work, but Windows SmartScreen and macOS Gatekeeper can warn
users. A public production release should be code-signed.

For macOS signing and notarization, configure these repository secrets:

- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`

Then set the repository variable `ENABLE_APPLE_SIGNING` to `true`. The release
workflow only forwards signing credentials to Tauri when this explicit switch
is enabled. Otherwise it creates unsigned installers, even if stale or partial
Apple secrets exist in the repository.

For Windows, obtain an Authenticode certificate or use Microsoft Trusted
Signing, then add the signing command according to the
[Tauri Windows signing guide](https://v2.tauri.app/distribute/sign/windows/).
Never commit certificates or passwords.

In-app updates use a separate Tauri signing key. Configure these repository
secrets for every release build:

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

The matching public key is intentionally embedded in `tauri.conf.json`. Keep an
offline backup of the updater private key and its password: losing it prevents
future releases from updating existing installations.

## Updating DeepSeek Harness

Harness is currently a developer preview and may make breaking changes. To
upgrade safely:

1. Change the exact version in `runtime/package.json`.
2. Update `HARNESS_VERSION` in `src-tauri/src/lib.rs`, `src/main.ts`, and
   `scripts/prepare-runtime.mjs`.
3. Regenerate `runtime/package-lock.json` with `npm install --package-lock-only`
   from the `runtime` directory.
4. Run the desktop smoke test on Windows, Apple Silicon, and Intel macOS.
5. Publish a new wrapper version instead of changing an existing release.

## License

The wrapper is MIT licensed. DeepSeek Harness, Node.js, Tauri, and bundled
dependencies retain their own licenses; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
