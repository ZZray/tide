# Third-party components / 第三方组件

Tide's original application source and brand assets are licensed under [MIT](LICENSE).
Dependencies retain their own licenses; Tide's MIT license does not relicense them.
Tide 原创应用代码和品牌资源使用 MIT；第三方组件仍遵循各自的许可证。

| Component | Purpose | License / source |
| --- | --- | --- |
| Rust standard library | Application runtime | MIT OR Apache-2.0, [Rust](https://github.com/rust-lang/rust) |
| Slint 1.16.1 | Desktop UI and software rendering | Desktop application use under [Slint Royalty-free License 2.0](https://github.com/slint-ui/slint/blob/v1.16.1/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md); AboutSlint is accessible in Tide's About screen |
| git2-rs / libgit2 | Git transport and history | [git2-rs](https://github.com/rust-lang/git2-rs) MIT/Apache-2.0; [libgit2](https://github.com/libgit2/libgit2) GPL-2.0 with linking exception |
| libssh2 | SSH transport | [BSD-style license](https://github.com/libssh2/libssh2/blob/master/COPYING) |
| notify | OS file events | [Artistic-2.0](https://github.com/notify-rs/notify) |
| ureq, native-tls, schannel, winreg, sha2, semver | Update checks and verified downloads | MIT / Apache-2.0, as recorded by Cargo metadata |
| tray-icon, muda, rfd, single-instance, auto-launch | Desktop integration | MIT / Apache-2.0 / other upstream license choices, as recorded by Cargo metadata |
| serde, toml, clap, image, chrono and transitive dependencies | Data, CLI, icons and time | License expressions and original notices are included in the generated license inventory |

The Windows release package includes `THIRD-PARTY-LICENSES.txt`, generated from
the exact locked dependency metadata and the license/notice files shipped with
the downloaded sources, including vendored native libraries. Missing package
notices use [reviewed upstream texts](licenses/README.md) pinned to their source
commit and content hash; unresolved notices fail packaging. The inventory may
include alternative licenses or build/test dependencies; listing an alternative
does not change the license choice described above. No dependency source code
is relicensed by the application license.

Windows 发布包随附 `THIRD-PARTY-LICENSES.txt`，从锁定依赖的元数据及源码内的许可、
版权声明生成，包含 vendored 原生库。清单可能包含多许可选项及构建/测试依赖，
不表示 Tide 选择了所有备选许可证。Slint 的 AboutSlint 组件会保留在发布版中。
