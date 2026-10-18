# Upstream license fallbacks / 上游许可补齐

Some published crates omit their standalone license files. `upstream/index.json`
maps each such **exact package version** to license/notice text retrieved from the
upstream repository at the commit recorded by that crate's `.cargo_vcs_info.json`.
Each entry records the source commit, source URLs and SHA-256 of the retained text.

部分 crates.io 源码包没有附带完整许可文件。`upstream/index.json` 按**精确包版本**记录
上游 `.cargo_vcs_info.json` 对应提交的许可/版权文本、来源 URL 和本地文本 SHA-256。

`scripts/collect-licenses.ps1` includes original package notices first, uses these
reviewed fallbacks only when none are shipped, and fails packaging if a required
fallback is absent, has a different license expression, or fails its hash check.
Updating a dependency may therefore require reviewing and updating this directory.
`simd_helpers` is an explicit exception: its upstream commit has a MIT declaration
and author attribution but no standalone license or copyright notice. Its fallback
retains that original declaration and the full canonical SPDX MIT text, with both
source commits recorded; no copyright year or ownership notice is invented.
Inactive/build/test dependencies may be included in the inventory as a conservative
superset; inclusion does not mean they are linked into the release executable.

打包优先使用源码包原文，仅在没有附带许可文件时使用这里的已核查文本；缺失、许可表达式
变化或哈希不符都会阻断打包。升级依赖时需同步复核此目录。清单保守包含部分未启用或构建/
测试依赖，不表示这些组件全部链接进发布程序。

`simd_helpers` 上游提交只有 MIT 声明及作者归属，没有独立许可或版权通知；此项明确保留
原始声明及完整 SPDX MIT 标准文本，记录两者来源提交，不编造版权年份或所有权声明。

These third-party texts retain their original licenses. The repository's MIT
license applies to Tide's own code and original assets, not to these upstream texts.
