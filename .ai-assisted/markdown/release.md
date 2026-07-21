发布流水线

职责 推送任意tag后构建常用桌面平台安装包，全部构建成功后汇总附件并创建GitHub Release草稿，发布说明由维护者补充后手动发布

入口 .github/workflows/release.yml

数据流 tag push -> 校验四处应用版本一致 -> Windows x64、Linux x64、macOS Intel与Apple Silicon并行打包 -> workflow artifacts -> 汇总为同tag发布草稿

附件 Windows生成MSI与NSIS，macOS生成两种架构DMG，Linux生成DEB、RPM与AppImage

特殊陷阱 tag指向的提交必须已包含工作流且package.json、package-lock.json、tauri.conf.json、Cargo.toml版本一致；Tauri bundle使用的PNG图标必须为RGBA而非索引色/调色板模式，否则Linux与macOS会在generate_context阶段报icon is not RGBA；草稿重跑会覆盖同名附件，已正式发布的Release禁止覆盖；当前未配置Windows代码签名与Apple签名公证，系统可能显示未知发布者或安全警告；Linux固定Ubuntu 22.04作为较低glibc基线
