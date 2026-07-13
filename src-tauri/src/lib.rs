mod commands;
mod ssh;

use ssh::manager::SessionManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(SessionManager::default())
        .invoke_handler(tauri::generate_handler![
            commands::ssh_connect,
            commands::ssh_disconnect,
            commands::terminal_open,
            commands::terminal_write,
            commands::terminal_resize,
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
            commands::sftp_set_sudo,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
