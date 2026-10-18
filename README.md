<p align="center"><img src="assets/brand/tide-logo.svg" width="390" alt="Tide"></p>
<p align="center">Git-powered document sync. Your folders, your remotes, your history.</p>
<p align="center"><b>English</b> · <a href="README.zh-CN.md">简体中文</a> · <a href="https://github.com/ZZray/tide/releases">Downloads</a></p>

Tide is a lightweight desktop and command-line tool that connects each local folder to a Git remote. It synchronizes changes, preserves both versions of conflicting documents, and keeps recoverable history. No Tide account or proprietary synchronization server is required.

**Status: v0.1.0 preview.** Windows x64 is the packaged desktop platform. The desktop interface is currently Chinese; this documentation and CLI help are available in English. Linux and macOS are source/CI targets, with no installer or tray support promised in this preview.

## Download and install

Get the cloud-built files from [GitHub Releases](https://github.com/ZZray/tide/releases):

- `Tide-v0.1.0-windows-x64-setup.exe`: per-user installer, Start menu shortcut and uninstaller; administrator rights are not required.
- `Tide-v0.1.0-windows-x64-portable.zip`: extract the entire folder and run `tide.exe`.
- `SHA256SUMS.txt`: SHA-256 checksums for both packages.

Preview packages are unsigned. Uninstalling the application preserves your documents, Git history and user configuration. Even the portable build stores configuration in the OS user configuration directory unless you pass `--config`.

## Start synchronizing

1. Open Tide and use **+** to select a local folder, Git remote and branch.
2. Preview the connection. For independent local/remote histories, inspect both sides before explicitly confirming the initial merge.
3. Choose an interval, change-triggered synchronization and conflict policy in **规则** (Rules), or use **立即同步** (Sync now).
4. Browse **历史** (History) to inspect differences, restore a folder version or export a historical file. The CLI also supports single-file restore.

Each folder is independent. The default conflict policy keeps the remote version at the original path and writes a separate local conflict copy. Restore/overwrite operations add commits; Tide never force-pushes or rebases your document history.

For HTTPS, configure a system Git credential helper. SSH supports an agent or an explicit key path. Tokens are not stored in Tide configuration; embedded credentials in HTTPS URLs are rejected. Tide uses libgit2 for Git I/O; an external credential helper may require system Git.

## Portable preferences

Settings can **export/import theme and notification preferences**. Put the exported file in a synchronized folder and explicitly import it on another device. Receiving the file does not automatically apply settings. Local paths, remotes, device names, credentials, autostart, synchronization rules and pause/guard states stay local. See [the portable preference format](docs/preferences-sync.md).

## CLI

```powershell
tide init
tide add notes 'D:\notes' 'git@github.com:you/notes.git' --branch main
tide plan notes
tide sync notes
tide watch
tide history notes
tide diff notes HEAD --path 'note.md'
tide restore notes <commit> --path 'note.md'
tide sync notes
tide preferences export 'D:\notes\.tide\preferences.toml'
tide preferences inspect 'D:\notes\.tide\preferences.toml'
tide preferences import 'D:\notes\.tide\preferences.toml'
```

Run `tide --help` for pause/resume, export, conflict resolution and recoverable overwrite commands. Starting without a command opens the desktop. `--config <path>` selects a configuration file. The desktop and `watch` share one background service and cannot run simultaneously for the same configuration.

## Technology

| Layer | Implementation |
| --- | --- |
| Application | Rust workspace: `tide-app`, `tide-core`, `tide-git` |
| Desktop | Slint 1.16.1, Winit backend, software renderer, system fonts; no bundled WebView |
| Git | `git2` with vendored libgit2; SSH/HTTPS transport; byte-preserving blobs and three-way text reconciliation |
| Coordination | Worker threads, per-folder serialization, at most two concurrent tasks, typed phase/failure events |
| Files and configuration | `notify`, TOML/serde, atomic writes, lock files, persistent write-recovery journal |
| Desktop integration | `tray-icon`, `rfd`, `single-instance`, `auto-launch`, `notify-rust` |
| Windows distribution | Embedded multi-resolution icon, static MSVC CRT, Inno Setup installer and portable ZIP, GitHub Actions builds |

## Build and contribute

Use stable Rust and a C/C++ toolchain. Windows needs MSVC Build Tools and a Windows SDK. Linux desktop build packages are listed in [CI](.github/workflows/ci.yml).

```sh
cargo build --release --workspace --locked
cargo test --workspace --locked -- --skip thousand_alternating_rounds_preserve_versions
cargo clippy --workspace --all-targets --locked -- -D warnings
# CLI-only:
cargo build -p tide-app --release --no-default-features --locked
```

[Development and packaging](docs/development.md) · [Contributing](CONTRIBUTING.md) · [Original design](docs/specs/2026-10-18_Tide设计.md) · [Implementation evidence and limits](docs/implementation/M2.md).

## Current limits

- Ask-mode conflicts and unstable/locked/oversized files stop that folder's current round. The default keeps both conflict versions.
- No Git LFS, symlinks/junctions, submodules, non-UTF-8 filenames or Windows-incompatible paths.
- Tracked files are checked as raw bytes; the original scan/memory/size performance targets are not all met or benchmarked.
- No built-in keyring/token UI, signed installer, automatic updater or dedicated sleep/wake integration.
- Hash checks and recoverable atomic replacement reduce write risk but cannot guarantee against every editor/filesystem race. Inspect safety pauses before resuming.

## License

Tide's original code and brand assets use the [MIT License](LICENSE). Third-party components retain their licenses. Slint is used under its royalty-free desktop application terms, with the **AboutSlint** widget in the accessible About screen. Release packages include dependency license texts and Rust library notices; see [third-party notices](THIRD_PARTY_NOTICES.md).
