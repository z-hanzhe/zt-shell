# 发布流水线

## 入口与产物

- `.github/workflows/release.yml` 在推送标签时执行发布流程。
- 工作流构建 Windows x64、macOS Intel、macOS Apple Silicon 和 Linux x64 安装包，并创建或更新对应标签的发布草稿。
- 产物包括 Windows MSI/NSIS、macOS DMG，以及 Linux DEB/RPM/AppImage。

## 发布约束

- 发布前会校验 `package.json`、`package-lock.json`、`src-tauri/tauri.conf.json` 和 `src-tauri/Cargo.toml` 中的版本一致。
- 已正式发布的 Release 不会被工作流覆盖；草稿可重跑并更新附件，发布说明须在正式发布前人工确认。
- ⚠️ 陷阱：应用 PNG 图标必须为 RGBA；使用索引色或调色板 PNG 会导致 Linux 或 macOS 打包失败。
