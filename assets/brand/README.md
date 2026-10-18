# Tide identity / Tide 品牌

Two parallel tidal curves represent independent devices converging on one continuous history.
两条平行潮线代表独立设备共享连续历史；保持简洁，在小尺寸托盘图标上也能辨认。

- `tide-logo.svg`: horizontal wordmark / 横向标志
- `tide-mark.svg`: transparent symbol / 透明符号
- `tide-icon.svg`, `tide-256.png`: application tile / 应用图标
- `tide-tray.png`: transparent tray icon / 托盘图标
- `tide.ico`: 16–256 px Windows icon / Windows 多尺寸图标

Palette: deep sea `#101E25`, tide cyan `#9BE0E7`, wordmark blue `#65B8C5`.
Use the cyan mark on dark backgrounds, or the application tile on light backgrounds.
Do not stretch, rotate, or add outlines. Rebuild PNG/ICO derivatives on Windows with
`pwsh ./scripts/generate-brand.ps1` (System.Drawing; no external image service).

Original assets are distributed under the repository's MIT license.
