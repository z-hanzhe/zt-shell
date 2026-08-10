//! 连接配置导入导出文件的原生选择与读写命令。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;

use crate::credentials::{CredentialKind, CredentialManager};

/** 连接导入导出文件允许的最大字节数。 */
const MAX_CONNECTION_FILE_SIZE: u64 = 10 * 1024 * 1024;
/** 未提供有效默认名称时使用的导出文件名。 */
const DEFAULT_EXPORT_FILE_NAME: &str = "ztshell-connections.json";

/// 一条导出记录与本地稳定标识的映射。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionExportCredentialSource {
    /// 导出 JSON 中连接或代理的文件内引用。
    export_ref: String,
    /// 系统凭据库使用的本地连接或代理稳定标识。
    source_id: String,
}

/// 导出内容使用的连接与代理凭据来源。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionExportCredentialSources {
    /// 导出连接引用与本地连接标识的映射。
    connections: Vec<ConnectionExportCredentialSource>,
    /// 导出代理引用与本地代理标识的映射。
    proxies: Vec<ConnectionExportCredentialSource>,
}

/// 导出 JSON 中待注入凭据的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportCredentialTarget {
    /// 密码认证连接的登录密码。
    ConnectionPassword(usize),
    /// 私钥认证连接的私钥口令。
    ConnectionPassphrase(usize),
    /// 支持密码认证的代理密码。
    ProxyPassword(usize),
}

/// 一项待从系统凭据库读取的导出凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportCredentialRequest {
    /// 凭据写入导出 JSON 的目标位置。
    target: ExportCredentialTarget,
    /// 系统凭据类别。
    kind: CredentialKind,
    /// 系统凭据库使用的本地稳定标识。
    source_id: String,
}

/// 检查文件大小是否超过连接导入导出的限制。
fn ensure_size_allowed(size: u64) -> Result<(), String> {
    if size > MAX_CONNECTION_FILE_SIZE {
        return Err("连接文件不能超过 10 MiB".to_string());
    }
    Ok(())
}

/// 判断路径是否使用 JSON 扩展名。
fn has_json_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

/// 将导出路径规范为 JSON 扩展名。
fn normalize_json_path(mut path: PathBuf) -> PathBuf {
    if !has_json_extension(&path) {
        path.set_extension("json");
    }
    path
}

/// 将前端提供的默认名称限制为单个 JSON 文件名。
fn normalize_default_file_name(default_file_name: &str) -> String {
    let trimmed = default_file_name.trim();
    let file_name = Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .unwrap_or(DEFAULT_EXPORT_FILE_NAME);
    normalize_json_path(PathBuf::from(file_name))
        .to_string_lossy()
        .into_owned()
}

/// 将前端凭据来源列表索引为导出引用到本地稳定标识的映射。
fn index_credential_sources(
    sources: Vec<ConnectionExportCredentialSource>,
    record_name: &str,
) -> Result<HashMap<String, String>, String> {
    let mut indexed = HashMap::with_capacity(sources.len());
    let mut source_ids = HashSet::with_capacity(sources.len());
    for source in sources {
        if source.export_ref.trim().is_empty() {
            return Err(format!("{record_name}凭据映射中的导出引用不能为空"));
        }
        if source.source_id.trim().is_empty() {
            return Err(format!("{record_name}凭据映射中的源标识不能为空"));
        }
        if !source_ids.insert(source.source_id.clone()) {
            return Err(format!(
                "{record_name}凭据映射重复使用源标识 [{}]",
                source.source_id
            ));
        }
        if indexed
            .insert(source.export_ref.clone(), source.source_id)
            .is_some()
        {
            return Err(format!(
                "{record_name}凭据映射包含重复导出引用 [{}]",
                source.export_ref
            ));
        }
    }
    Ok(indexed)
}

/// 获取导出文档中的对象数组。
fn export_array_mut<'a>(
    document: &'a mut Value,
    field: &str,
) -> Result<&'a mut Vec<Value>, String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "连接导出内容的根节点必须是对象".to_string())?;
    root.get_mut(field)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("连接导出内容中的 [{field}] 必须是数组"))
}

/// 从一条导出记录中读取非空字符串字段。
fn read_non_empty_text(
    record: &Map<String, Value>,
    field: &str,
    record_path: &str,
) -> Result<String, String> {
    let value = record
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("连接导出内容中的 [{record_path}.{field}] 必须是字符串"))?;
    if value.trim().is_empty() {
        return Err(format!("连接导出内容中的 [{record_path}.{field}] 不能为空"));
    }
    Ok(value.to_string())
}

/// 确认全部凭据来源都引用了导出文档中的实际记录。
fn ensure_sources_consumed(
    remaining: HashMap<String, String>,
    record_name: &str,
) -> Result<(), String> {
    if remaining.is_empty() {
        return Ok(());
    }
    let mut references: Vec<String> = remaining.into_keys().collect();
    references.sort();
    Err(format!(
        "{record_name}凭据映射引用了不存在的导出记录 [{}]",
        references.join(", ")
    ))
}

/// 解析无秘密导出 JSON、校验来源映射并生成系统凭据读取计划。
fn prepare_connection_export(
    content: &str,
    credential_sources: ConnectionExportCredentialSources,
) -> Result<(Value, Vec<ExportCredentialRequest>), String> {
    let mut document: Value = serde_json::from_str(content)
        .map_err(|error| format!("连接导出内容不是有效的 JSON：{error}"))?;
    let mut connection_sources = index_credential_sources(credential_sources.connections, "连接")?;
    let mut proxy_sources = index_credential_sources(credential_sources.proxies, "代理")?;
    let mut requests = Vec::new();
    let mut used_proxy_refs = HashSet::new();

    {
        let connections = export_array_mut(&mut document, "connections")?;
        let mut seen_refs = HashSet::with_capacity(connections.len());
        for (index, value) in connections.iter_mut().enumerate() {
            let path = format!("connections[{index}]");
            let record = value
                .as_object_mut()
                .ok_or_else(|| format!("连接导出内容中的 [{path}] 必须是对象"))?;
            let export_ref = read_non_empty_text(record, "ref", &path)?;
            if !seen_refs.insert(export_ref.clone()) {
                return Err(format!("连接导出内容包含重复连接引用 [{export_ref}]"));
            }
            let source_id = connection_sources
                .remove(&export_ref)
                .ok_or_else(|| format!("缺少连接 [{export_ref}] 的凭据来源映射"))?;
            let auth_type = read_non_empty_text(record, "authType", &path)?;

            // 无条件丢弃渲染层内容中的秘密，只允许系统凭据库覆盖目标字段。
            record.insert("password".to_string(), Value::Null);
            record.insert("passphrase".to_string(), Value::Null);
            match auth_type.as_str() {
                "password" => requests.push(ExportCredentialRequest {
                    target: ExportCredentialTarget::ConnectionPassword(index),
                    kind: CredentialKind::ConnectionPassword,
                    source_id,
                }),
                "privateKey" => requests.push(ExportCredentialRequest {
                    target: ExportCredentialTarget::ConnectionPassphrase(index),
                    kind: CredentialKind::ConnectionPassphrase,
                    source_id,
                }),
                _ => {
                    return Err(format!("连接导出内容中的 [{path}.authType] 不受支持"));
                }
            }

            match record.get("proxyRef") {
                Some(Value::String(proxy_ref)) if !proxy_ref.trim().is_empty() => {
                    used_proxy_refs.insert(proxy_ref.clone());
                }
                Some(Value::Null) => {}
                Some(_) => {
                    return Err(format!(
                        "连接导出内容中的 [{path}.proxyRef] 必须是非空字符串或 null"
                    ));
                }
                None => return Err(format!("连接导出内容缺少 [{path}.proxyRef]")),
            }
        }
    }
    ensure_sources_consumed(connection_sources, "连接")?;

    {
        let proxies = export_array_mut(&mut document, "proxies")?;
        let mut seen_refs = HashSet::with_capacity(proxies.len());
        for (index, value) in proxies.iter_mut().enumerate() {
            let path = format!("proxies[{index}]");
            let record = value
                .as_object_mut()
                .ok_or_else(|| format!("连接导出内容中的 [{path}] 必须是对象"))?;
            let export_ref = read_non_empty_text(record, "ref", &path)?;
            if !seen_refs.insert(export_ref.clone()) {
                return Err(format!("连接导出内容包含重复代理引用 [{export_ref}]"));
            }
            if !used_proxy_refs.remove(&export_ref) {
                return Err(format!("导出代理 [{export_ref}] 未被任何连接使用"));
            }
            let source_id = proxy_sources
                .remove(&export_ref)
                .ok_or_else(|| format!("缺少代理 [{export_ref}] 的凭据来源映射"))?;
            let proxy_type = read_non_empty_text(record, "proxyType", &path)?;

            record.insert("password".to_string(), Value::Null);
            match proxy_type.as_str() {
                "socks5" | "http" => requests.push(ExportCredentialRequest {
                    target: ExportCredentialTarget::ProxyPassword(index),
                    kind: CredentialKind::ProxyPassword,
                    source_id,
                }),
                "socks4" | "socks4a" => {}
                _ => {
                    return Err(format!("连接导出内容中的 [{path}.proxyType] 不受支持"));
                }
            }
        }
    }
    if !used_proxy_refs.is_empty() {
        let mut references: Vec<String> = used_proxy_refs.into_iter().collect();
        references.sort();
        return Err(format!(
            "连接引用了未包含在导出内容中的代理 [{}]",
            references.join(", ")
        ));
    }
    ensure_sources_consumed(proxy_sources, "代理")?;
    Ok((document, requests))
}

/// 将已解析的可选凭据写入对应导出记录，不存在的凭据保持为 `null`。
fn apply_export_credentials(
    document: &mut Value,
    credentials: Vec<(ExportCredentialTarget, Option<String>)>,
) -> Result<(), String> {
    for (target, secret) in credentials {
        let (collection, index, field) = match target {
            ExportCredentialTarget::ConnectionPassword(index) => ("connections", index, "password"),
            ExportCredentialTarget::ConnectionPassphrase(index) => {
                ("connections", index, "passphrase")
            }
            ExportCredentialTarget::ProxyPassword(index) => ("proxies", index, "password"),
        };
        let record = export_array_mut(document, collection)?
            .get_mut(index)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "连接导出凭据写入位置无效".to_string())?;
        record.insert(
            field.to_string(),
            secret.map(Value::String).unwrap_or(Value::Null),
        );
    }
    Ok(())
}

/// 弹出原生文件选择框并读取一个 UTF-8 JSON 连接导入文件，取消时返回 `None`。
#[tauri::command]
pub async fn pick_connection_import_file(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("导入连接")
        .add_filter("JSON 文件", &["json"])
        .pick_file(move |selected| {
            let _ = sender.send(selected);
        });

    let Some(selected) = receiver
        .await
        .map_err(|_| "连接导入文件选择框异常关闭".to_string())?
    else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|error| format!("无法读取所选连接导入路径：{error}"))?;
    if !has_json_extension(&path) {
        return Err("连接导入文件必须使用 .json 扩展名".to_string());
    }

    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|error| format!("读取连接导入文件信息失败：{error}"))?;
    if !metadata.is_file() {
        return Err("所选连接导入路径不是文件".to_string());
    }
    ensure_size_allowed(metadata.len())?;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| format!("打开连接导入文件失败：{error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CONNECTION_FILE_SIZE + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("读取连接导入文件失败：{error}"))?;
    ensure_size_allowed(bytes.len() as u64)?;

    let content =
        String::from_utf8(bytes).map_err(|_| "连接导入文件不是有效的 UTF-8 文本".to_string())?;
    Ok(Some(
        content
            .strip_prefix('\u{feff}')
            .unwrap_or(&content)
            .to_string(),
    ))
}

/// 弹出原生保存框并以 UTF-8 无 BOM 写入连接导出文件，取消时返回 `false`。
#[tauri::command]
pub async fn save_connection_export_file(
    app: AppHandle,
    credentials: State<'_, CredentialManager>,
    content: String,
    default_file_name: String,
    credential_sources: ConnectionExportCredentialSources,
) -> Result<bool, String> {
    let content = content
        .strip_prefix('\u{feff}')
        .map(str::to_owned)
        .unwrap_or(content);
    ensure_size_allowed(content.len() as u64)?;
    let (mut document, requests) = prepare_connection_export(&content, credential_sources)?;
    let default_file_name = normalize_default_file_name(&default_file_name);
    let (sender, receiver) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("导出连接")
        .set_file_name(default_file_name)
        .add_filter("JSON 文件", &["json"])
        .save_file(move |selected| {
            let _ = sender.send(selected);
        });

    let Some(selected) = receiver
        .await
        .map_err(|_| "连接导出文件保存框异常关闭".to_string())?
    else {
        return Ok(false);
    };
    let path = selected
        .into_path()
        .map_err(|error| format!("无法读取所选连接导出路径：{error}"))?;
    let path = normalize_json_path(path);

    let mut resolved = Vec::with_capacity(requests.len());
    for request in requests {
        let secret = credentials
            .get_optional(request.kind, &request.source_id)
            .await
            .map_err(|error| error.to_string())?;
        resolved.push((request.target, secret));
    }
    apply_export_credentials(&mut document, resolved)?;
    let mut output = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("生成连接导出文件失败：{error}"))?;
    output.push('\n');
    ensure_size_allowed(output.len() as u64)?;
    tokio::fs::write(path, output.as_bytes())
        .await
        .map_err(|error| format!("写入连接导出文件失败：{error}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_export_credentials, ensure_size_allowed, has_json_extension,
        normalize_default_file_name, normalize_json_path, prepare_connection_export,
        ConnectionExportCredentialSource, ConnectionExportCredentialSources,
        ExportCredentialRequest, ExportCredentialTarget, MAX_CONNECTION_FILE_SIZE,
    };
    use crate::credentials::CredentialKind;
    use serde_json::Value;
    use std::path::{Path, PathBuf};

    /// 构造测试使用的连接与代理凭据来源映射。
    fn credential_sources(
        connections: &[(&str, &str)],
        proxies: &[(&str, &str)],
    ) -> ConnectionExportCredentialSources {
        ConnectionExportCredentialSources {
            connections: connections
                .iter()
                .map(|(export_ref, source_id)| ConnectionExportCredentialSource {
                    export_ref: (*export_ref).to_string(),
                    source_id: (*source_id).to_string(),
                })
                .collect(),
            proxies: proxies
                .iter()
                .map(|(export_ref, source_id)| ConnectionExportCredentialSource {
                    export_ref: (*export_ref).to_string(),
                    source_id: (*source_id).to_string(),
                })
                .collect(),
        }
    }

    /// JSON 扩展名检查应忽略大小写并拒绝其他格式。
    #[test]
    fn checks_json_extension() {
        assert!(has_json_extension(Path::new("connections.json")));
        assert!(has_json_extension(Path::new("connections.JSON")));
        assert!(!has_json_extension(Path::new("connections.txt")));
        assert!(!has_json_extension(Path::new("connections")));
    }

    /// 导出路径缺少 JSON 扩展名时应自动补齐。
    #[test]
    fn normalizes_export_extension() {
        assert_eq!(
            normalize_json_path(PathBuf::from("connections")),
            PathBuf::from("connections.json")
        );
        assert_eq!(
            normalize_json_path(PathBuf::from("connections.JSON")),
            PathBuf::from("connections.JSON")
        );
    }

    /// 默认文件名不得携带目录且必须使用 JSON 扩展名。
    #[test]
    fn normalizes_default_export_file_name() {
        assert_eq!(
            normalize_default_file_name("nested/connections.txt"),
            "connections.json"
        );
        assert_eq!(normalize_default_file_name(""), "ztshell-connections.json");
    }

    /// 文件大小上限允许恰好 10 MiB，并拒绝多出的任意字节。
    #[test]
    fn checks_connection_file_size() {
        assert!(ensure_size_allowed(MAX_CONNECTION_FILE_SIZE).is_ok());
        assert!(ensure_size_allowed(MAX_CONNECTION_FILE_SIZE + 1).is_err());
    }

    /// 来源映射应按认证与代理类型读取并注入正确的系统凭据。
    #[test]
    fn maps_and_injects_export_credentials() {
        let content = r#"{
            "connections": [
                {
                    "ref": "connection-password-ref",
                    "authType": "password",
                    "password": "不得采用渲染层密码",
                    "passphrase": "不得采用渲染层口令",
                    "privateKeyPath": null,
                    "proxyRef": "proxy-ref"
                },
                {
                    "ref": "connection-key-ref",
                    "authType": "privateKey",
                    "password": "不得采用渲染层密码",
                    "passphrase": "不得采用渲染层口令",
                    "privateKeyPath": "C:\\Keys\\id_ed25519",
                    "proxyRef": null
                }
            ],
            "proxies": [{
                "ref": "proxy-ref",
                "proxyType": "socks5",
                "password": "不得采用渲染层代理密码"
            }]
        }"#;
        let (mut document, requests) = prepare_connection_export(
            content,
            credential_sources(
                &[
                    ("connection-password-ref", "connection-password-source"),
                    ("connection-key-ref", "connection-key-source"),
                ],
                &[("proxy-ref", "proxy-source")],
            ),
        )
        .unwrap();

        assert_eq!(
            requests,
            vec![
                ExportCredentialRequest {
                    target: ExportCredentialTarget::ConnectionPassword(0),
                    kind: CredentialKind::ConnectionPassword,
                    source_id: "connection-password-source".to_string(),
                },
                ExportCredentialRequest {
                    target: ExportCredentialTarget::ConnectionPassphrase(1),
                    kind: CredentialKind::ConnectionPassphrase,
                    source_id: "connection-key-source".to_string(),
                },
                ExportCredentialRequest {
                    target: ExportCredentialTarget::ProxyPassword(0),
                    kind: CredentialKind::ProxyPassword,
                    source_id: "proxy-source".to_string(),
                },
            ]
        );
        apply_export_credentials(
            &mut document,
            vec![
                (
                    ExportCredentialTarget::ConnectionPassword(0),
                    Some("系统连接密码".to_string()),
                ),
                (
                    ExportCredentialTarget::ConnectionPassphrase(1),
                    Some("系统私钥口令".to_string()),
                ),
                (
                    ExportCredentialTarget::ProxyPassword(0),
                    Some("系统代理密码".to_string()),
                ),
            ],
        )
        .unwrap();

        assert_eq!(
            document.pointer("/connections/0/password"),
            Some(&Value::String("系统连接密码".to_string()))
        );
        assert_eq!(
            document.pointer("/connections/0/passphrase"),
            Some(&Value::Null)
        );
        assert_eq!(
            document.pointer("/connections/1/password"),
            Some(&Value::Null)
        );
        assert_eq!(
            document.pointer("/connections/1/passphrase"),
            Some(&Value::String("系统私钥口令".to_string()))
        );
        assert_eq!(
            document.pointer("/connections/1/privateKeyPath"),
            Some(&Value::String("C:\\Keys\\id_ed25519".to_string()))
        );
        assert_eq!(
            document.pointer("/proxies/0/password"),
            Some(&Value::String("系统代理密码".to_string()))
        );
    }

    /// 系统凭据不存在时导出字段应保持为 null。
    #[test]
    fn keeps_export_secrets_null_when_credentials_are_absent() {
        let content = r#"{
            "connections": [{
                "ref": "connection-ref",
                "authType": "password",
                "password": "不得采用渲染层密码",
                "passphrase": null,
                "proxyRef": "proxy-ref"
            }],
            "proxies": [{
                "ref": "proxy-ref",
                "proxyType": "http",
                "password": "不得采用渲染层代理密码"
            }]
        }"#;
        let (mut document, requests) = prepare_connection_export(
            content,
            credential_sources(
                &[("connection-ref", "connection-source")],
                &[("proxy-ref", "proxy-source")],
            ),
        )
        .unwrap();
        let absent = requests
            .into_iter()
            .map(|request| (request.target, None))
            .collect();
        apply_export_credentials(&mut document, absent).unwrap();

        assert_eq!(
            document.pointer("/connections/0/password"),
            Some(&Value::Null)
        );
        assert_eq!(
            document.pointer("/connections/0/passphrase"),
            Some(&Value::Null)
        );
        assert_eq!(document.pointer("/proxies/0/password"), Some(&Value::Null));
    }

    /// 凭据来源映射不得引用导出文档中不存在的记录。
    #[test]
    fn rejects_unknown_export_credential_reference() {
        let content = r#"{
            "connections": [{
                "ref": "connection-ref",
                "authType": "password",
                "password": null,
                "passphrase": null,
                "proxyRef": null
            }],
            "proxies": []
        }"#;
        let error = prepare_connection_export(
            content,
            credential_sources(
                &[
                    ("connection-ref", "connection-source"),
                    ("unknown-ref", "unknown-source"),
                ],
                &[],
            ),
        )
        .unwrap_err();
        assert!(error.contains("不存在的导出记录 [unknown-ref]"));
    }
}
