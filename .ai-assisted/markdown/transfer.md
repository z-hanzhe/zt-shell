传输任务管理

实现 TransferManager为Tauri托管状态 树形结构 文件夹聚合节点(进度由子任务汇总) 文件任务执行单元 Semaphore限3文件并发 其余排队 start_progress_loop节流推送

事件 transfer://changed(结构变化推全量) transfer://progress(每300ms增量推送 仅变化项) 速度指数平滑 已完成以total/elapsed算平均 目录自底向上聚合状态(转译/running>packing>pending>paused>failed>completed) running/packing累计dt暂停不计时 任务按sessionId归属 ssh_disconnect清理全部

断点续传 started_once标记区分首次/续传 首次TRUNCATE 续传open_with_flags(WRITE|CREATE不含TRUNCATE)配合seek实现offset写 上传stat远端大小、下载stat本地大小后seek接续 下载EOF不足total报错交重试 失败自动重试3次间隔2s 超次标记failed 控制AtomicU8下发暂停/取消

数量校验 本次+会话未完成>100直接拒绝(推荐打包) >50且非force返回needConfirm确认后重调(force仍受100上限) 覆盖检测存在同名返回existNames确认后overwrite重调 文件夹覆盖合并写入 上传本地枚举spawn_blocking显式栈遍历(防递归栈溢出) 远端目录ensure_remote_dir逐级建立+DashSet缓存(一次上传共享祖先) create_upload清缓存避免复用过期记录

打包下载 exec探测tar后远端打包(哨兵__ZTOK__/__ZTFAIL__)为单文件下载 packing态exec不可中断(打包结束检查点落地) 完成后清理临时包 失败保留供重试续传(大小一致跳过打包) 仅Linux sudo模式不提权

命令 transfer_upload/download/create_pack_download/list/pause/resume/remove/retry_failed 参数与返回见api.ts

前端 stores/transfers.ts监听changed+progress事件 App.vue初始化 TransferPanel.vue按sessionId过滤 树展开收起 列参数均布局可拖拽 键盘导航/多选/右键菜单动态切项 与FileManager上传下载入口联动

限制 内存态(重启不保留) 会话断开清理全部 删除任务不清理半成品文件
