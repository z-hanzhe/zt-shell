发布流水线

入口 .github/workflows/release.yml(tauri-apps/tauri-action由Tauri官方维护 用于跨平台bundle识别与artifacts上传)

数据流 tag push -> 校验四处应用版本一致(package.json/package-lock.json/tauri.conf.json/Cargo.toml) -> Windows x64、Linux x64、macOS Intel与Apple Silicon并行打包 -> workflow artifacts -> tauri-action汇总为同tag发布草稿

附件 Windows MSI+NSIS macOS双架构DMG Linux DEB+RPM+AppImage

陷阱 PNG图标必须RGBA非索引色/调色板 否则Linux/macOS generate_context报icon is not RGBA 草稿重跑覆盖同名附件 正式Release禁止覆盖 无代码签名/公证 系统显示未知发布者 Linux固定Ubuntu 22.04低glibc基线
