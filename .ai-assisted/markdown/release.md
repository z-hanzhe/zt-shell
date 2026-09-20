# 发布流水线

## 入口与产物

- `.github/workflows/ci.yml` 在分支推送和拉取请求时检查前端测试、发布契约与生产构建，并于 Linux 运行 Rust 格式检查、测试与 Clippy；推送标签不会触发。
- `.github/workflows/release.yml` 在推送稳定版标签时构建四个平台，全部成功后自动正式发布。
- `scripts/release.mjs` 负责版本与说明校验、按架构归档、签名密钥标识检查和官方静态更新清单生成。
- `release-notes/<版本>.md` 是 GitHub Release 与应用内更新说明的共同来源。
- 产物包括 Windows MSI/NSIS、macOS Intel 与 Apple Silicon DMG/应用归档，以及 Linux DEB/RPM/AppImage；更新清单按架构和原安装器类型分发。

## 发布约束

- 标签须与各清单及锁文件中的应用版本一致；正式发布前必须存在对应版本的有效更新说明。
- 所有平台的安装包和签名齐全后才公开 Release；失败的草稿可以重跑，已正式发布的版本不可覆盖。
- 公钥保存在 Tauri 配置中；本机构建由 `scripts/tauri.mjs` 映射签名环境变量，GitHub Actions 使用同名 Repository secrets，配置入口见项目 README。
- 私钥及口令只用于构建环境；新增来源必须继续使用相同的签名校验契约，客户端运行约束见 [软件更新](updater.md)。
- CLI 包装入口只向需要签名的命令传递凭据；帮助、版本和非签名命令必须清除签名环境变量。
- ⚠️ 陷阱：应用 PNG 图标必须为 RGBA；使用索引色或调色板 PNG 会导致 Linux 或 macOS 打包失败。
