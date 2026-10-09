//! 软件更新边界：固定更新源、明确代理策略并持有已验证的安装包。

use std::time::Duration;

use reqwest::Proxy;
use serde::{Deserialize, Serialize};
use serde_json::from_value;
use tauri::{
    async_runtime::spawn_blocking, ipc::Channel, AppHandle, Manager, State, Url, WebviewWindow,
};
use tauri_plugin_updater::{Config as UpdaterConfig, Update, UpdaterExt};
use tokio::sync::Mutex;

use crate::credentials::{CredentialKind, CredentialManager};

/// 更新状态只在主进程持有，关闭设置页不会中断下载。
#[derive(Default)]
pub struct UpdateState(Mutex<Option<PendingUpdate>>);

/// 检查结果和通过官方签名校验的安装包。
struct PendingUpdate {
    update: Update,
    bytes: Option<Vec<u8>>,
}

/// 支持的更新源；新增源时须同步前端选项和跨端类型。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateSource {
    Github,
    #[serde(rename = "gh-proxy.com")]
    GhProxyCom,
    #[serde(rename = "ghproxy.net")]
    GhproxyNet,
    #[serde(rename = "ghfast.top")]
    GhfastTop,
}

impl UpdateSource {
    /// 清单与安装包使用同一加速前缀，官方源保留原始地址。
    fn resource_url(&self, url: &Url) -> Result<Url, String> {
        let prefix = match self {
            Self::Github => return Ok(url.clone()),
            Self::GhProxyCom => "https://gh-proxy.com/",
            Self::GhproxyNet => "https://ghproxy.net/",
            Self::GhfastTop => "https://ghfast.top/",
        };
        if url.scheme() != "https" || url.host_str() != Some("github.com") {
            return Err("加速更新源仅支持 GitHub HTTPS 资源地址".into());
        }
        Url::parse(&format!("{prefix}{url}")).map_err(|_| "更新加速地址格式不正确".to_string())
    }
}

/// 更新代理的非秘密配置，密码只允许从系统凭据库读取。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProxy {
    url: String,
    #[serde(default)]
    username: String,
    credential_id: Option<String>,
}

/// 应用运行信息，用于阻止开发环境覆盖已安装的软件。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEnvironment {
    version: String,
    can_install: bool,
}

/// 前端展示的更新信息，不包含可修改的下载地址和签名。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: String,
    date: Option<String>,
}

/// 下载进度；只有下载命令成功返回才代表签名验证通过。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
}

/// 将更新操作限制在主窗口。
fn ensure_main_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("请在主窗口检查和安装更新".into());
    }
    Ok(())
}

/// 校验代理地址，避免把认证信息明文写入更新偏好。
fn parse_proxy(value: &str) -> Result<Url, String> {
    let url = Url::parse(value.trim()).map_err(|_| "代理地址格式不正确".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("请填写不含账号、密码或路径的 HTTP / HTTPS 代理地址".into());
    }
    Ok(url)
}

/// 创建携带独立认证头的代理，避免在代理地址和日志中出现密码。
fn authenticated_proxy(config: &UpdateProxy, password: Option<&str>) -> Result<Proxy, String> {
    let url = parse_proxy(&config.url)?;
    let username = config.username.trim();
    if username.contains(':') {
        return Err("代理用户名不能包含冒号".into());
    }
    if username.is_empty() && password.is_some() {
        return Err("请填写代理用户名，或清除已保存的代理密码".into());
    }
    let proxy = Proxy::all(url).map_err(|_| "代理地址格式不正确".to_string())?;
    Ok(if username.is_empty() {
        proxy
    } else {
        proxy.basic_auth(username, password.unwrap_or_default())
    })
}

/// 获取当前版本及当前运行环境的安装能力。
#[tauri::command]
pub fn updater_environment(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<UpdateEnvironment, String> {
    ensure_main_window(&window)?;
    Ok(UpdateEnvironment {
        version: app.package_info().version.to_string(),
        can_install: !cfg!(debug_assertions),
    })
}

/// 检查官方清单，关闭代理时显式禁用系统与环境代理。
#[tauri::command]
pub async fn updater_check(
    window: WebviewWindow,
    app: AppHandle,
    source: UpdateSource,
    proxy: Option<UpdateProxy>,
    state: State<'_, UpdateState>,
    credentials: State<'_, CredentialManager>,
) -> Result<Option<UpdateInfo>, String> {
    ensure_main_window(&window)?;
    let mut pending = state.0.try_lock().map_err(|_| "更新操作正在进行中")?;
    let mut builder = app
        .updater_builder()
        .timeout(Duration::from_secs(30))
        .no_proxy();
    if !matches!(source, UpdateSource::Github) {
        // 共用配置中的官方清单和公钥，加速源只转换传输地址。
        let config = app
            .config()
            .plugins
            .0
            .get("updater")
            .ok_or("未找到更新配置")?;
        let config: UpdaterConfig =
            from_value(config.clone()).map_err(|error| format!("更新配置不正确：{error}"))?;
        let endpoints = config
            .endpoints
            .iter()
            .map(|url| source.resource_url(url))
            .collect::<Result<Vec<_>, _>>()?;
        builder = builder
            .endpoints(endpoints)
            .map_err(|error| format!("更新源配置不正确：{error}"))?;
    }
    if let Some(config) = proxy {
        let password = if let Some(id) = &config.credential_id {
            if !id.starts_with("updater-proxy:") {
                return Err("更新代理凭据标识不正确，请重新保存代理密码".into());
            }
            Some(
                credentials
                    .get_optional(CredentialKind::ProxyPassword, id)
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or("未找到已保存的更新代理密码，请重新填写并保存")?,
            )
        } else {
            None
        };
        let proxy = authenticated_proxy(&config, password.as_deref())?;
        // 同一个客户端配置回调用于检查及下载，认证不会进入源站请求头。
        builder = builder.configure_client(move |client| client.no_proxy().proxy(proxy.clone()));
    }
    let mut result = builder
        .build()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| format!("检查更新失败：{error}"))?;
    if let Some(update) = result.as_mut() {
        // 仅转换下载地址，继续由官方插件使用原清单签名验证安装包。
        update.download_url = source.resource_url(&update.download_url)?;
        // 检查与下载共享同一代理；大文件使用独立的下载超时。
        update.timeout = Some(Duration::from_secs(30 * 60));
    }
    let info = result.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        date: update.date.map(|date| date.to_string()),
    });
    *pending = result.map(|update| PendingUpdate {
        update,
        bytes: None,
    });
    Ok(info)
}

/// 下载指定版本并由官方插件验证签名，拒绝过期的前端操作。
#[tauri::command]
pub async fn updater_download(
    window: WebviewWindow,
    version: String,
    on_progress: Channel<UpdateProgress>,
    state: State<'_, UpdateState>,
) -> Result<(), String> {
    ensure_main_window(&window)?;
    if cfg!(debug_assertions) {
        return Err("开发环境不下载更新，请使用已安装的正式版本".into());
    }
    let mut pending = state.0.try_lock().map_err(|_| "更新操作正在进行中")?;
    let pending = pending.as_mut().ok_or("请先检查更新")?;
    if pending.update.version != version {
        return Err("版本信息已变化，请重新检查更新".into());
    }
    if pending.bytes.is_some() {
        return Ok(());
    }
    let mut downloaded = 0_u64;
    pending.bytes = Some(
        pending
            .update
            .download(
                |length, total| {
                    downloaded += length as u64;
                    let _ = on_progress.send(UpdateProgress { downloaded, total });
                },
                || {},
            )
            .await
            .map_err(|error| format!("下载或验证更新失败：{error}"))?,
    );
    Ok(())
}

/// 在前端完成退出保护后安装；Windows 由安装器重启，其他平台主动重启。
#[tauri::command]
pub async fn updater_install(
    window: WebviewWindow,
    app: AppHandle,
    version: String,
) -> Result<(), String> {
    ensure_main_window(&window)?;
    if cfg!(debug_assertions) {
        return Err("开发环境不允许安装更新".into());
    }
    spawn_blocking(move || {
        let state = app.state::<UpdateState>();
        let pending = state.0.try_lock().map_err(|_| "更新操作正在进行中")?;
        let pending = pending.as_ref().ok_or("请先检查更新")?;
        if pending.update.version != version {
            return Err("版本信息已变化，请重新检查更新".to_string());
        }
        let bytes = pending.bytes.as_ref().ok_or("请先下载更新")?;
        pending
            .update
            .install(bytes)
            .map_err(|error| format!("安装更新失败：{error}"))?;
        #[cfg(not(target_os = "windows"))]
        app.restart();
        #[cfg(target_os = "windows")]
        Ok(())
    })
    .await
    .map_err(|error| format!("安装任务失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use reqwest::Client;
    use rustls::crypto::ring::default_provider;
    use serde_json::{from_str, from_value, Value};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        spawn,
        time::timeout,
    };

    use super::{
        authenticated_proxy, parse_proxy, Duration, UpdateProxy, UpdateSource, UpdaterConfig, Url,
    };

    /// 四个前端来源标识必须能读取同一清单，并为各平台安装包保留版本及编码路径。
    #[test]
    fn routes_manifest_and_platform_downloads_through_selected_source() {
        let config: Value = from_str(include_str!("../tauri.conf.json")).unwrap();
        let config: UpdaterConfig = from_value(config["plugins"]["updater"].clone()).unwrap();
        assert_eq!(config.endpoints.len(), 1);
        let manifest = &config.endpoints[0];
        assert_eq!(
            manifest.as_str(),
            "https://github.com/z-hanzhe/zt-shell/releases/latest/download/latest.json"
        );
        let sources = [
            ("github", ""),
            ("gh-proxy.com", "https://gh-proxy.com/"),
            ("ghproxy.net", "https://ghproxy.net/"),
            ("ghfast.top", "https://ghfast.top/"),
        ];
        let assets = [
            "ZTShell_0.2.9_windows-x86_64-setup.exe",
            "ZTShell_0.2.9_windows-x86_64.msi",
            "ZTShell_0.2.9_darwin-aarch64.app.tar.gz",
            "ZTShell_0.2.9_darwin-x86_64.app.tar.gz",
            "ZTShell_0.2.9_linux-x86_64.AppImage",
            "ZTShell_0.2.9_linux-x86_64.deb",
            "ZTShell_0.2.9_linux-x86_64.rpm",
            "ZTShell%20%E5%AE%89%E8%A3%85%E5%8C%85%2B.zip",
        ];
        for (id, prefix) in sources {
            let source: UpdateSource = from_value(Value::String(id.into())).unwrap();
            assert_eq!(
                source.resource_url(manifest).unwrap().as_str(),
                format!("{prefix}https://github.com/z-hanzhe/zt-shell/releases/latest/download/latest.json")
            );
            for asset in assets {
                let original = format!(
                    "https://github.com/z-hanzhe/zt-shell/releases/download/v0.2.9/{asset}"
                );
                let download = Url::parse(&original).unwrap();
                assert_eq!(
                    source.resource_url(&download).unwrap().as_str(),
                    format!("{prefix}{original}")
                );
            }
        }
        assert!(from_value::<UpdateSource>(Value::String("unknown".into())).is_err());
    }

    /// 加速源拒绝不受支持的协议和域名，避免拼接出无法使用的代理地址。
    #[test]
    fn rejects_unsupported_accelerated_resource_urls() {
        for source in [
            UpdateSource::GhProxyCom,
            UpdateSource::GhproxyNet,
            UpdateSource::GhfastTop,
        ] {
            for value in [
                "http://github.com/z-hanzhe/zt-shell/releases/download/v0.2.9/package.zip",
                "https://example.com/package.zip",
                "https://github.com.example.com/package.zip",
            ] {
                assert!(source.resource_url(&Url::parse(value).unwrap()).is_err());
            }
        }
    }

    /// 检查和下载均通过代理隧道认证，特殊字符不得被当作地址分隔符。
    #[tokio::test]
    async fn authenticates_proxy_tunnels_without_exposing_credentials() {
        // 正式检查由更新插件安装加密实现，独立客户端测试显式执行相同初始化。
        let _ = default_provider().install_default();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = UpdateProxy {
            url: format!("http://{}", listener.local_addr().unwrap()),
            username: "用户@/%".into(),
            credential_id: None,
        };
        let proxy = authenticated_proxy(&config, Some("密:码 @/%")).unwrap();
        let proxy_debug = format!("{proxy:?}");
        assert!(!proxy_debug.contains("密:码"));
        assert!(!proxy_debug.contains("55So5oi3QC8lOuWvhjrnoIEgQC8l"));
        let server = spawn(async move {
            for host in ["check.invalid", "download.invalid"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert!(count > 0 && request.len() < 8192);
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with(&format!("CONNECT {host}:443 HTTP/1.1\r\n")));
                assert!(request.lines().any(|line| {
                    line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("proxy-authorization")
                            && value.trim() == "Basic 55So5oi3QC8lOuWvhjrnoIEgQC8l"
                    })
                }));
                // 截止于认证层验证，不连接外网或安装更新。
                stream
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .unwrap();
            }
        });
        for host in ["check.invalid", "download.invalid"] {
            let client = Client::builder()
                .no_proxy()
                .proxy(proxy.clone())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            assert!(client
                .get(format!("https://{host}/update"))
                .send()
                .await
                .is_err());
        }
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    /// 空密码可用于仅用户名认证，缺少用户名的密码与非法用户名会被拒绝。
    #[test]
    fn validates_proxy_authentication() {
        let mut config = UpdateProxy {
            url: "https://proxy.example:8080".into(),
            username: "user".into(),
            credential_id: None,
        };
        assert!(authenticated_proxy(&config, None).is_ok());
        config.username.clear();
        assert!(authenticated_proxy(&config, Some("password")).is_err());
        assert!(authenticated_proxy(&config, None).is_ok());
        config.username = "invalid:user".into();
        assert!(authenticated_proxy(&config, Some("password")).is_err());
    }

    /// 代理开关开启时只接受合法的无认证 HTTP 代理。
    #[test]
    fn validates_proxy_address() {
        assert!(parse_proxy("http://127.0.0.1:7897").is_ok());
        assert!(parse_proxy("https://proxy.example:8080/").is_ok());
        for value in [
            "",
            "127.0.0.1:7897",
            "file:///tmp",
            "http://user:secret@localhost",
            "http://localhost/path",
            "http://localhost?token=secret",
        ] {
            assert!(parse_proxy(value).is_err(), "应拒绝代理地址：{value}");
        }
    }
}
