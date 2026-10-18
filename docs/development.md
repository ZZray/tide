# Development and releases / 开发与发布

## Architecture

`tide-git` owns byte-preserving snapshots, fetch/push, reconciliation and the write
journal. `tide-core` owns validation, preferences, run records, watching and task
coordination. `tide-app` provides the Slint desktop and CLI. GUI and CLI watch share
the same service; they do not maintain separate synchronization algorithms.

Git 层负责原始字节、网络、对账和写回记录；Core 负责配置、偏好、记录、监听、调度；
App 提供桌面与 CLI。界面不另写同步算法。所有写操作沿用文件夹串行和恢复协议。

## Local prerequisites

Stable Rust is used by CI. Windows requires an MSVC C++ toolchain and Windows SDK.
Ubuntu desktop dependencies: `libgtk-3-dev libayatana-appindicator3-dev libxdo-dev
libxkbcommon-dev libfontconfig1-dev libssl-dev pkg-config`. Standard-library headers
must be available when building vendored native libraries.

The workspace declares Rust 1.89, but the minimum version is not an independently
tested compatibility promise; CI validates current stable and the exact Cargo.lock.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- --skip thousand_alternating_rounds_preserve_versions
cargo build --release --workspace --locked
```

The 1000-round real Git test is available through `Synchronization stress test`
in GitHub Actions, or `cargo test -p tide-git --release --locked
thousand_alternating_rounds_preserve_versions -- --nocapture`.

## This Windows workstation / 本机缓存

`scripts/dev.ps1` is an optional workstation helper, not required by contributors
or CI. It loads the local MSVC environment and existing Rust tools from
`D:\workspace\env\tools\rust` and applies:

| Data | Location |
| --- | --- |
| Cargo registry/git package cache | `F:\cache\rust\cargo` |
| Compiled artifact cache | `F:\cache\rust\target` |
| Intermediate build state | `F:\cache\rust\build\{workspace-path-hash}` |
| Rust child-process scratch files | `F:\cache\rust\tmp` |

Successful `cargo build` through the helper exports `tide.exe` to this project's
`target/debug` or `target/release`. Package caches and intermediates stay on F:.
Hosted GitHub runners use their own temporary/workspace volume; they do not require
an F: drive. On other machines use Cargo directly or choose your own cache roots.

## Cloud Windows packages

[Windows packages](../.github/workflows/release.yml) runs on `windows-latest`:

1. Validate the tag against the workspace version, run formatting, Clippy and tests.
2. Build the application, embed its icon and collect dependency/Rust license notices.
3. Compile the per-user Inno Setup installer and create a portable ZIP plus checksums.
4. Install into an isolated directory, execute the installed CLI and uninstall it.
5. Upload artifacts; a version tag publishes a GitHub preview release after verification.

Manual workflow dispatch builds artifacts without publishing a release. Release
tags are `v<workspace version>`, with bilingual notes in `docs/releases/<tag>.md`.
CI builds do not sign binaries. No signing certificate or long-lived publishing
token is committed; GitHub's scoped job token publishes release assets.

云端实际编译、打包和安装/卸载检查均在托管 Windows runner 上执行。
手动运行只生成构建附件；版本 tag 才发布预览 Release。安装器按用户安装，
不删除用户文档、配置或 Git 历史。签名不属于本预览版发布流程。

Local packaging requires PowerShell 7, Inno Setup 6 and a release build:

```powershell
./scripts/package-windows.ps1 -Executable ./target/release/tide.exe -OutputDirectory ./target/dist-local
./scripts/test-windows-package.ps1 -Installer ./target/dist-local/Tide-v0.1.0-windows-x64-setup.exe
```

Original brand SVGs live in `assets/brand`. `scripts/generate-brand.ps1` rebuilds
their PNG/ICO derivatives with Windows System.Drawing.
