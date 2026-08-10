mod commands;
mod connection_file;
mod credentials;
mod ssh;

use credentials::CredentialManager;
use ssh::host_keys::HostKeyStore;
use ssh::manager::SessionManager;
use ssh::transfer::TransferManager;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单例插件必须最先注册：再次启动时唤起并聚焦已运行的窗口，禁止多开
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                // 最小化时先还原，再置顶并聚焦，确保已存在实例被显示到前台
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(CredentialManager::default())
        .manage(SessionManager::default())
        .manage(TransferManager::default())
        .setup(|app| {
            let known_hosts_path = app.path().app_data_dir()?.join("known_hosts.json");
            app.manage(HostKeyStore::new(known_hosts_path));
            // 启动传输进度节流推送循环
            ssh::transfer::start_progress_loop(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connection_file::pick_connection_import_file,
            connection_file::save_connection_export_file,
            commands::credentials_set_many,
            commands::credentials_check_many,
            commands::credentials_match_many,
            commands::credentials_delete_many,
            commands::credentials_copy_many,
            commands::ssh_connect,
            commands::ssh_disconnect,
            commands::terminal_open,
            commands::terminal_write,
            commands::terminal_resize,
            commands::path_is_dir,
            commands::monitor_collect,
            commands::sftp_list,
            commands::sftp_home,
            commands::sftp_read,
            commands::sftp_write,
            commands::sftp_remove_file,
            commands::sftp_remove_dir,
            commands::sftp_create_dir,
            commands::sftp_rename,
            commands::sftp_upload,
            commands::sftp_download,
            commands::sftp_create_archive,
            commands::sftp_extract_archive,
            commands::sftp_remove_entries,
            commands::sftp_cancel_operation,
            commands::sftp_set_sudo,
            commands::sftp_check_writable,
            commands::transfer_upload,
            commands::transfer_download,
            commands::transfer_pack_download,
            commands::transfer_list,
            commands::transfer_pause,
            commands::transfer_resume,
            commands::transfer_remove,
            commands::transfer_retry_failed,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
