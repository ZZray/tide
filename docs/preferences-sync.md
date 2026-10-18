# Portable preferences / 可同步偏好

Tide 0.1 shares **theme and notification preference only**. Export from Settings,
save the TOML inside a folder already synchronized by Tide, then explicitly import
it on the other device. Saving or receiving the file does not automatically apply it.

Tide 0.1 仅共享**主题和通知偏好**。在“设置”中导出，保存到已同步的文件夹，
然后在另一设备的“设置”中手动导入。导入会先显示将要应用的值；收到文件本身不会自动改变设置。

```toml
version = 1
theme = "system" # system / light / dark
notify = "errors" # errors / every / off
```

CLI:

```powershell
tide preferences export 'D:\notes\.tide\preferences.toml'
tide preferences inspect 'D:\notes\.tide\preferences.toml'
tide preferences import 'D:\notes\.tide\preferences.toml'
```

Export refuses to overwrite an existing file. Choose a new file or explicitly
remove the old exported file yourself. Import rejects unknown fields, unsupported
versions and files over 64 KiB. Full `config.toml` is deliberately not accepted.

导出不覆盖已有文件；请选择新文件名，或自行明确删除旧导出文件。
导入拒绝未知字段、不支持的版本及超过 64 KiB 的文件，不接受完整 `config.toml`。

| Shared / 共享 | Kept local / 保持本机独立 |
| --- | --- |
| Theme, notification preference / 主题、通知偏好 | Device name, folder IDs, paths, remotes, branches / 设备名、文件夹 ID、路径、远端、分支 |
| | Credentials, SSH key paths, autostart / 凭据、私钥路径、开机启动 |
| | Close-to-tray, pause/guard state, concurrency / 关闭留托盘、暂停/护栏状态、并发数 |
| | Ignore/conflict/timing rules, history location / 忽略/冲突/调度规则、历史目录 |

This is an opt-in file sharing mechanism, not a hosted account or settings service.
The portable file is plaintext and stored in the Git remote you select. Never place
the machine configuration or credentials into a public synchronization repository.

这是主动选择的文件共享机制，不需要 Tide 云账号。偏好文件是明文，保存在用户选择的 Git 远端。
不要把本机完整配置或凭据放进公开同步仓库。
