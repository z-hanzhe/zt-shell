# Tauri 命令

## 职责与入口

- `src-tauri/src/commands.rs` 是前端与 Rust SSH 内核之间的命令适配层，统一将内部错误转为字符串结果。
- `src/api.ts` 提供前端调用封装；命令参数和返回类型分别以 `src/types.ts`、`ssh/types.rs` 为准。
- `src-tauri/src/lib.rs` 注册命令、Tauri 插件和托管状态。

## 命令分组

- SSH 与终端：连接、主机密钥确认、断开、打开终端、读写终端和调整终端尺寸。
- 文件与监控：目录和文件操作、sudo 切换、操作取消、远端监控采集。
- 传输：创建上传/下载/打包下载任务，以及列表、暂停、继续、删除和失败重试。

## 变更约束

- 新增命令时同步更新 Rust 实现、`commands.rs`、`lib.rs` 注册、`api.ts` 和跨端类型；需要原生窗口或插件能力时同步检查 `src-tauri/capabilities/`。
- SSH 建连返回“已连接”或“需要确认主机密钥”；确认结果出现时尚未建立后端会话，前端须携带所展示的完整公钥重新建连。
- `SessionManager` 与 `TransferManager` 通过 Tauri `manage` 注入；新增共享运行时状态应遵循相同托管方式。
- 主窗口与独立编辑器窗口拥有不同 capability；新增窗口能力必须同时核对窗口配置与权限范围。
