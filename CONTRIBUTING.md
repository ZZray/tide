# Contributing / 参与贡献

Open an issue with the expected behavior, actual behavior, OS, Tide version and
a small reproducible example. Remove personal paths, remote credentials and
document contents from logs before posting. For code changes, use a focused pull
request, keep `Cargo.lock` updated and run:

提交问题时提供预期行为、实际行为、系统、Tide 版本和最小复现。公开日志前移除个人路径、
凭据和文档内容。代码贡献请使用范围清晰的 PR，维护 Cargo.lock，并运行：

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- --skip thousand_alternating_rounds_preserve_versions
```

Run the 1000-round regression when changing synchronization semantics; it is also
available through the manually dispatched GitHub stress workflow. Test byte
preservation, both conflict versions and interrupted-write recovery when relevant.
Do not add force pushes, silently migrate user repositories, or store credentials
in configuration. Keep the AboutSlint widget in the top-level About screen.

修改同步语义时应额外运行 1000 轮回归；云端也提供手动触发的压力测试工作流。
涉及字节、冲突或写回时补充相应回归。不得加入强推、静默迁移历史或明文存储凭据；
保留顶层“关于”中的 AboutSlint 组件。原创贡献按 MIT 提供，第三方代码需保留原始许可。
