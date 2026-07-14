传输任务管理

实现 见src-tauri/src/ssh/transfer.rs TransferManager为Tauri托管状态 任务树结构 文件夹为聚合节点(进度状态由子任务汇总)文件任务为执行单元 全局Semaphore限3个文件并发 其余排队 lib.rs的setup启动start_progress_loop节流推送

事件 transfer://changed结构变化(创建删除)推全量任务列表 transfer://progress每300ms推动态字段增量(仅变化项) 速度为指数平滑 目录节点每tick自底向上聚合transferred/total/speed/status 状态聚合优先级running>packing>pending>paused>failed>completed 经过时间在tick内对running/packing任务累计dt暂停不计时

断点续传 任务级started_once标记区分首次与续传 首次覆盖写(上传TRUNCATE/下载set_len(0)) 续传按已落盘字节定位 上传stat远端大小 下载stat本地大小 双端seek后接续 russh-sftp的File实现AsyncRead/Write/Seek open_with_flags(WRITE|CREATE不含TRUNCATE)配合seek实现offset写 下载读到提前EOF且不足total判定中断报错交给重试

重试与控制 失败自动重试3次间隔2s按断点续传 超次标记failed 手动"重试失败的作业"復位为pending续传 控制用AtomicU8 control(暂停/取消)传输循环每64KB块检查 pending任务暂停直接置paused running任务下发CTL_PAUSE由执行体落地 删除置ST_CANCELLED+移除 使排队runner自行退出

枚举与目录 上传本地枚举spawn_blocking显式栈遍历 下载远端SFTP read_dir显式栈遍历 相对路径统一正斜杠父先于子 数量校验check_file_count 本次文件数+未完成文件任务数(不分上传下载)>300直接拒绝报错 >100且force=false不建任务返回needConfirm由前端确认(红色"坚持传输"按钮)后force重调 force重调仍受300上限约束 远端目录由文件任务开始时ensure_remote_dir逐级建立带DashSet缓存 空目录单独spawn_dir_creator建目录后即completed

打包下载 create_pack_download先exec探测command -v tar 失败即报错 远端tar -czf /tmp/ztshell-{uuid}.tar.gz打包(cd目标目录 文件名shell单引号转义防注入 哨兵__ZTOK__/__ZTFAIL__判成败)后作为单文件下载任务 packing状态打包中exec不可中断暂停取消在打包结束检查点落地 下载完成/取消/删除exec rm -f清理远端临时包 失败保留临时包供重试续传(重试时tmp大小与记录一致则跳过打包直接续传下载 否则重新打包且本地从0下载) 走登录用户exec通道 sudo文件管理模式下打包不提权 仅Linux

命令 transfer_upload(sessionId,localPaths,remoteDir,force) transfer_download(sessionId,items[{path,isDir}],localDir,force)均返回{needConfirm,fileCount} transfer_pack_download(sessionId,remoteDir,names,localPath) transfer_list transfer_pause/transfer_resume/transfer_remove(ids可选null为全部 目录自动展开子树 删除级联) transfer_retry_failed

前端 stores/transfers.ts监听两事件 changed全量替换 progress按id经代理元素合并保证响应式 activeCount统计非目录任务pending/running/packing数量 uploadDoneTick上传任务转completed时递增供FileManager watch后600ms防抖refresh App.vue onMounted初始化 TransferPanel.vue传输面板 树形展开收起(expandedIds) 十列(文件名称/传输状态/传输进度/文件大小/本地路径/操作类型/远程路径/传输速度/预计剩余/经过时间) 单元格用td内.cell-flex容器布局勿在td上直接display:flex否则破坏table-cell垂直居中 列宽可拖拽 Ctrl/Shift/框选与FileManager同模式 右键菜单[暂停/全部暂停/继续/全部继续/删除/全部删除/重试失败的作业/清空已完成的任务]按选中与全局状态禁用 清空已完成删除顶层completed任务后端级联子树 双击目录行切换展开 BottomPanel传输选项卡右上角标显示activeCount上限99

FileManager入口 右键菜单上传文件/上传文件夹(plugin-dialog open multiple/directory 系统对话框无法混选文件与文件夹故分两项) 下载(选中≥1 open选本地目录) 打包下载(save默认名单选为name.tar.gz多选为目录名.tar.gz) needConfirm时弹"共N个文件,超过100个文件建议打包压缩后传输"[取消/坚持传输(红色btn-danger)] F5刷新当前目录(keydown用capture注册避开App层浏览器快捷键拦截) 系统文件拖入上传getCurrentWebview().onDragDropEvent 物理坐标除devicePixelRatio换算后判断落点在file-list区域内 over显示.drop-overlay虚线遮罩(挂在.file-list-wrap定位容器上避免随滚动位移) drop调startUpload复用数量校验 支持文件/多文件/目录混合拖入

已知限制 会话断开后旧任务重试因sessionId失效报会话不存在 需删除后重新创建 应用重启任务列表不保留(内存态) 残留半成品文件删除任务时不清理本地/远端已写部分
