SFTP文件管理

实现 见src-tauri/src/ssh/sftp.rs 基于russh-sftp的SftpSession 惰性建立 首次使用文件管理时open_sftp_channel请求sftp子系统再SftpSession::new

操作 list_dir列举目录返回FileEntry列表 目录在前文件在后按名排序 read_file读取 write_file创建或截断后覆盖写 remove_file删文件 remove_dir删空目录 create_dir建目录 rename重命名移动 canonicalize解析绝对路径用于定位主目录 upload本地文件读入再写远端 download远端读入再写本地

权限解析 format_permissions将FileType与mode转为drwxr-xr-x风格字符串

前端 见FileManager.vue 顶部路径栏含上级刷新新建目录 右侧列表非根目录置顶...返回上级 手动输入路径跳转 下载用tauri-plugin-dialog选择本地路径 增删改重命名 会话切换或连接成功后sftpHome定位主目录并联动左侧树

注意点 远端路径统一正斜杠 见utils.ts joinPath parentPath 目录暂不支持下载
