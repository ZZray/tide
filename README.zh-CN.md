<p align="center"><img src="assets/brand/tide-logo.svg" width="390" alt="Tide"></p>
<p align="center">基于 Git 的文档同步。自己的文件夹、远端与版本历史。</p>
<p align="center"><a href="README.md">English</a> · <b>简体中文</b> · <a href="https://github.com/ZZray/tide/releases">下载安装包</a></p>

Tide 是轻量桌面与命令行工具，把每个本地文件夹关联到 Git 远端，自动同步修改、保留冲突的双方内容，并提供可恢复的版本历史。不需要 Tide 云账号，也不依赖专有同步服务器。

**当前为 v0.1.1 预览版。** 发布 Windows x64 桌面安装程序和便携包，界面为中文。Linux/macOS 为源码与 CI 构建目标，本预览版不承诺其安装包或托盘功能。

## 下载与安装

从 [GitHub Releases](https://github.com/ZZray/tide/releases) 下载云端构建的文件：

- `Tide-v0.1.1-windows-x64-setup.exe`：按当前用户安装，提供开始菜单快捷方式和卸载程序，不要求管理员权限。安装版会自动更新。
- `Tide-v0.1.1-windows-x64-portable.zip`：完整解压后运行 `tide.exe`。
- `SHA256SUMS.txt`：两个发布包的 SHA-256 校验值。

预览包尚未签名。卸载程序保留用户文档、Git 历史和配置。便携版默认也将配置保存在系统用户配置目录；可用 `--config` 指定其他位置。

## 开始同步

1. 打开 Tide，点击 **+**，选择本地文件夹、Git 远端地址和分支。
2. 先预览连接。若两端是独立历史，检查内容后再明确确认首次合并。
3. 在 **规则** 中设置间隔、改动触发和冲突策略，或点击 **立即同步**。
4. 在 **历史** 中查看差异、恢复整个文件夹版本或导出历史文件。CLI 还支持恢复单个文件。

每个文件夹独立工作。默认“两份都保留”：原位置使用远端版本，本地版本另存为冲突副本。恢复和覆盖都追加提交，不强推、不 rebase 文档历史。

HTTPS 使用系统 Git 凭据助手；SSH 支持 agent 或显式私钥路径。Tide 不在配置中保存令牌，并拒绝内嵌凭据的 HTTPS URL。Git 操作由 libgit2 完成，外部凭据助手可能需要系统 Git。

## 自动更新

安装版启动约一分钟后检查一次 [GitHub Releases](https://github.com/ZZray/tide/releases)，之后每天一次。新版本在后台下载，并按同一发布的 `SHA256SUMS.txt` 校验；只有在侧栏、“关于”或托盘菜单点 **重启以更新** 后才会安装。正在进行的同步会先完成，然后静默安装并重新打开 Tide。每日检查可在设置中关闭，“关于”里随时可以手动检查。便携版只提示新版本并给出发布页。

检查读取公开的发布源（不走有限流的 REST API），使用 `HTTPS_PROXY`/`HTTP_PROXY` 或 Windows 系统代理。每次检查、下载和安装都记录在 `%LOCALAPPDATA%\Tide\Tide\cache\updates\update.log`。

## 部分配置也可以共享

在“设置”中**导出/导入主题和通知偏好**。把偏好文件放进已同步的文件夹，在另一设备手动导入即可；收到文件本身不会自动应用。路径、远端、设备名、凭据、开机启动、同步规则和暂停/护栏状态始终保持本机独立。详见 [偏好文件协议](docs/preferences-sync.md)。

## 命令行

```powershell
tide init
tide add 笔记 'D:\notes' 'git@github.com:you/notes.git' --branch main
tide plan 笔记
tide sync 笔记
tide watch
tide history 笔记
tide diff 笔记 HEAD --path '记录.md'
tide restore 笔记 <commit> --path '记录.md'
tide sync 笔记
tide preferences export 'D:\notes\.tide\preferences.toml'
tide preferences inspect 'D:\notes\.tide\preferences.toml'
tide preferences import 'D:\notes\.tide\preferences.toml'
tide update check
tide update install
```

`tide --help` 列出暂停/继续、历史导出、冲突选择和可撤销覆盖等命令。无参数启动桌面，`--config <路径>` 选择配置文件。桌面和 `watch` 共用后台服务，同一配置不能同时启动两者。

## 使用的技术

| 层次 | 技术与职责 |
| --- | --- |
| 应用 | Rust workspace，分为 `tide-app`、`tide-core`、`tide-git` |
| 界面 | Slint 1.16.1、Winit、软件渲染和系统字体，不打包 WebView |
| Git | `git2` + vendored libgit2，SSH/HTTPS，原始字节 blob，文本三方对账 |
| 后台 | 工作线程、每文件夹串行、全局最多两个并发任务，类型化阶段/错误事件 |
| 文件与配置 | `notify` 系统监听、TOML/serde、原子写入、配置锁和持久写回恢复记录 |
| 系统集成 | `tray-icon`、`rfd`、`single-instance`、`auto-launch`、`notify-rust` |
| 更新 | GitHub 发布源，`ureq` + Windows TLS 与代理设置，SHA-256 校验，Inno Setup 静默升级 |
| Windows 发布 | 内嵌多尺寸图标、静态 MSVC CRT、Inno Setup 安装程序、便携 ZIP、GitHub Actions 云端构建 |

## 构建与贡献

使用 stable Rust 和 C/C++ 编译工具链。Windows 需要 MSVC Build Tools 与 Windows SDK；Linux 桌面构建依赖见 [CI 工作流](.github/workflows/ci.yml)。

```sh
cargo build --release --workspace --locked
cargo test --workspace --locked -- --skip thousand_alternating_rounds_preserve_versions
cargo clippy --workspace --all-targets --locked -- -D warnings
# 只构建 CLI：
cargo build -p tide-app --release --no-default-features --locked
```

[开发、缓存和打包](docs/development.md) · [贡献说明](CONTRIBUTING.md) · [原始设计](docs/specs/2026-10-18_Tide设计.md) · [实现证据与限制](docs/implementation/M2.md) · [云端发布记录](docs/implementation/M3.md)。

## 当前限制

- Ask 冲突和不稳定、占用、超限文件会停止该文件夹当前轮；默认策略会保留冲突双方。
- 不支持 Git LFS、符号链接/junction、子模块、非 UTF-8 文件名或 Windows 不兼容命名。
- 为原始字节安全会复核已跟踪文件；最初的扫描、内存、体积目标尚未全部达到或完成基准测试。
- 没有内置钥匙串/令牌界面、签名安装程序或专门的睡眠唤醒集成。自动更新只适用于 Windows 安装版。
- 哈希核对和可恢复的原子写回减少覆盖风险，但不能保证覆盖所有编辑器/文件系统并发情况；解除安全暂停前需检查原因。

## MIT 与第三方许可

Tide 原创代码与品牌资源使用 [MIT 协议](LICENSE)。第三方组件保留各自许可。Slint 按免版税桌面应用条款使用，顶层可达的“关于”页保留 **AboutSlint**。发布包附带依赖许可文本和 Rust 标准库版权声明，见 [第三方说明](THIRD_PARTY_NOTICES.md)。
