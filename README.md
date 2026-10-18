# Tide

基于 Git 的轻量文档同步工具（暂定名）。常驻托盘，把多个本地文件夹各自接到一个 Git 远端，定时或改动后自动同步，所有版本留在历史里，覆盖和回滚都能撤销。

当前阶段：**设计草案，尚未实现。**

- 设计规格：[docs/specs/2026-10-18_Tide设计.md](docs/specs/2026-10-18_Tide设计.md)
- 界面样机：[docs/design/tide-design.html](docs/design/tide-design.html)（浏览器直接打开）

计划技术路线：Rust + Slint + libgit2（静态链接），支持 Windows / macOS / Linux。
