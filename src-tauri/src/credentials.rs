//! 系统凭据库访问边界：保存连接密码、私钥口令和代理密码

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use tokio::task;

use crate::ssh::types::{AuthType, ConnectionConfig};

/// 系统凭据库中的服务名称，与应用标识保持一致
const CREDENTIAL_SERVICE: &str = "site.hanzhe.zt-shell";

/// 系统凭据库管理器
#[derive(Default)]
pub struct CredentialManager {
    /// 串行化原生凭据操作，规避部分平台对同一凭据并发访问的顺序不确定性
    access: Arc<Mutex<()>>,
}

/// 系统凭据类别
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CredentialKind {
    /// SSH 密码认证使用的登录密码
    ConnectionPassword,
    /// SSH 私钥认证使用的私钥口令
    ConnectionPassphrase,
    /// SOCKS5 或 HTTP 代理使用的代理密码
    ProxyPassword,
}

impl CredentialKind {
    /// 返回凭据账户名使用的稳定类别标识
    fn account_prefix(self) -> &'static str {
        match self {
            Self::ConnectionPassword => "connection-password",
            Self::ConnectionPassphrase => "connection-passphrase",
            Self::ProxyPassword => "proxy-password",
        }
    }

    /// 返回适合错误提示的凭据名称
    fn display_name(self) -> &'static str {
        match self {
            Self::ConnectionPassword => "连接密码",
            Self::ConnectionPassphrase => "私钥口令",
            Self::ProxyPassword => "代理密码",
        }
    }
}

/// 系统凭据定位键
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialKey {
    /// 凭据类别
    pub kind: CredentialKind,
    /// 连接或代理的稳定标识
    pub id: String,
}

/// 待写入系统凭据库的凭据
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialWrite {
    /// 凭据类别
    pub kind: CredentialKind,
    /// 连接或代理的稳定标识
    pub id: String,
    /// 凭据明文，仅在本次写入期间存在
    pub value: String,
}

/// 代理密码匹配参数
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMatch {
    /// 凭据类别
    pub kind: CredentialKind,
    /// 连接或代理的稳定标识
    pub id: String,
    /// 仅在本次比较期间存在的候选明文
    pub value: String,
}

/// 系统凭据复制参数
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialCopy {
    /// 凭据类别
    pub kind: CredentialKind,
    /// 来源连接或代理标识
    pub source_id: String,
    /// 目标连接或代理标识
    pub target_id: String,
}

/// 校验并生成系统凭据账户名
fn credential_account(kind: CredentialKind, id: &str) -> Result<String> {
    if id.trim().is_empty() {
        return Err(anyhow!("{}标识不能为空", kind.display_name()));
    }
    Ok(format!("v1:{}:{}", kind.account_prefix(), id))
}

/// 创建一项系统凭据句柄
fn credential_entry(key: &CredentialKey) -> Result<Entry> {
    let account = credential_account(key.kind, &key.id)?;
    Entry::new(CREDENTIAL_SERVICE, &account)
        .map_err(|error| anyhow!("初始化{}存储失败：{}", key.kind.display_name(), error))
}

/// 同步读取一项凭据，不存在时返回空
fn read_credential(key: &CredentialKey) -> Result<Option<String>> {
    let entry = credential_entry(key)?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(anyhow!(
            "读取系统凭据库中的{}失败：{}",
            key.kind.display_name(),
            error
        )),
    }
}

/// 同步读取一项必需凭据
fn read_required_credential(key: &CredentialKey) -> Result<String> {
    read_credential(key)?.ok_or_else(|| {
        anyhow!(
            "未找到已保存的{}，请重新填写并保存",
            key.kind.display_name()
        )
    })
}

/// 同步写入一项凭据
fn write_credential(change: CredentialWrite) -> Result<()> {
    let key = CredentialKey {
        kind: change.kind,
        id: change.id,
    };
    credential_entry(&key)?
        .set_password(&change.value)
        .map_err(|error| anyhow!("保存{}失败：{}", key.kind.display_name(), error))
}

/// 同步删除一项凭据，不存在时按幂等成功处理
fn delete_credential(key: &CredentialKey) -> Result<()> {
    let entry = credential_entry(key)?;
    match entry.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(anyhow!("删除{}失败：{}", key.kind.display_name(), error)),
    }
}

impl CredentialManager {
    /// 在阻塞线程中串行执行系统凭据操作
    async fn run_blocking<T, F>(&self, operation_name: &'static str, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        let access = Arc::clone(&self.access);
        task::spawn_blocking(move || {
            let _guard = access
                .lock()
                .map_err(|_| anyhow!("系统凭据库访问状态异常，请重启应用后重试"))?;
            operation()
        })
        .await
        .map_err(|error| anyhow!("{}任务执行失败：{}", operation_name, error))?
    }

    /// 批量写入系统凭据
    pub async fn set_many(&self, changes: Vec<CredentialWrite>) -> Result<()> {
        self.run_blocking("写入系统凭据", move || {
            for change in changes {
                write_credential(change)?;
            }
            Ok(())
        })
        .await
    }

    /// 批量检查系统凭据是否存在，结果顺序与输入一致
    pub async fn check_many(&self, keys: Vec<CredentialKey>) -> Result<Vec<bool>> {
        self.run_blocking("检查系统凭据", move || {
            keys.iter()
                .map(|key| read_credential(key).map(|value| value.is_some()))
                .collect()
        })
        .await
    }

    /// 批量比较代理密码，结果顺序与输入一致且不返回已存明文
    pub async fn match_many(&self, changes: Vec<CredentialMatch>) -> Result<Vec<bool>> {
        if changes
            .iter()
            .any(|change| change.kind != CredentialKind::ProxyPassword)
        {
            return Err(anyhow!("系统凭据比较仅支持代理密码"));
        }
        self.run_blocking("比较系统凭据", move || {
            changes
                .into_iter()
                .map(|change| {
                    let key = CredentialKey {
                        kind: change.kind,
                        id: change.id,
                    };
                    let stored = read_credential(&key)?;
                    Ok(stored.as_deref() == Some(change.value.as_str()))
                })
                .collect()
        })
        .await
    }

    /// 批量删除系统凭据
    pub async fn delete_many(&self, keys: Vec<CredentialKey>) -> Result<()> {
        self.run_blocking("删除系统凭据", move || {
            for key in &keys {
                delete_credential(key)?;
            }
            Ok(())
        })
        .await
    }

    /// 批量复制系统凭据，来源不存在时返回明确错误
    pub async fn copy_many(&self, changes: Vec<CredentialCopy>) -> Result<()> {
        self.run_blocking("复制系统凭据", move || {
            for change in changes {
                let source = CredentialKey {
                    kind: change.kind,
                    id: change.source_id,
                };
                let value = read_required_credential(&source)?;
                write_credential(CredentialWrite {
                    kind: change.kind,
                    id: change.target_id,
                    value,
                })?;
            }
            Ok(())
        })
        .await
    }

    /// 读取一项可选凭据
    pub async fn get_optional(&self, kind: CredentialKind, id: &str) -> Result<Option<String>> {
        let key = CredentialKey {
            kind,
            id: id.to_owned(),
        };
        self.run_blocking("读取系统凭据", move || read_credential(&key))
            .await
    }

    /// 按保存记录标识向 SSH 建连配置注入系统凭据
    pub async fn inject_connection_credentials(
        &self,
        config: &mut ConnectionConfig,
        saved_connection_id: String,
        saved_proxy_id: Option<String>,
    ) -> Result<()> {
        // 无条件丢弃渲染层传入的秘密，只允许使用系统凭据库中的值。
        config.password = None;
        config.passphrase = None;
        if let Some(proxy) = config.proxy.as_mut() {
            proxy.password = None;
        }

        let connection_kind = match config.auth_type {
            AuthType::Password => Some(CredentialKind::ConnectionPassword),
            AuthType::PrivateKey if config.has_passphrase => {
                Some(CredentialKind::ConnectionPassphrase)
            }
            AuthType::PrivateKey => None,
        };

        let proxy_kind = match (&config.proxy, saved_proxy_id.as_deref()) {
            (Some(proxy), Some(proxy_id)) if proxy.id != proxy_id => {
                return Err(anyhow!("连接使用的代理标识与已保存代理不一致"));
            }
            (Some(proxy), Some(_)) if proxy.has_password => Some(CredentialKind::ProxyPassword),
            (Some(proxy), None) if proxy.has_password => {
                return Err(anyhow!("缺少已保存代理标识，无法读取代理密码"));
            }
            (None, Some(_)) => return Err(anyhow!("未找到连接使用的代理配置")),
            _ => None,
        };

        let connection_key = connection_kind.map(|kind| CredentialKey {
            kind,
            id: saved_connection_id,
        });
        let proxy_key = match (proxy_kind, saved_proxy_id) {
            (Some(kind), Some(id)) => Some(CredentialKey { kind, id }),
            _ => None,
        };

        let (connection_secret, proxy_secret) = self
            .run_blocking("读取建连凭据", move || {
                let connection_secret = connection_key
                    .as_ref()
                    .map(read_required_credential)
                    .transpose()?;
                let proxy_secret = proxy_key
                    .as_ref()
                    .map(read_required_credential)
                    .transpose()?;
                Ok((connection_secret, proxy_secret))
            })
            .await?;

        match connection_kind {
            Some(CredentialKind::ConnectionPassword) => config.password = connection_secret,
            Some(CredentialKind::ConnectionPassphrase) => config.passphrase = connection_secret,
            _ => {}
        }
        if let Some(proxy) = config.proxy.as_mut() {
            proxy.password = proxy_secret;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 凭据类别必须使用约定的 camelCase 值跨端传输
    #[test]
    fn serializes_credential_kinds_as_camel_case() {
        assert_eq!(
            serde_json::to_string(&CredentialKind::ConnectionPassword).unwrap(),
            "\"connectionPassword\""
        );
        assert_eq!(
            serde_json::to_string(&CredentialKind::ConnectionPassphrase).unwrap(),
            "\"connectionPassphrase\""
        );
        assert_eq!(
            serde_json::to_string(&CredentialKind::ProxyPassword).unwrap(),
            "\"proxyPassword\""
        );
    }

    /// 不同类别与标识必须生成互不冲突的账户名
    #[test]
    fn builds_stable_distinct_accounts() {
        let password = credential_account(CredentialKind::ConnectionPassword, "same-id").unwrap();
        let passphrase =
            credential_account(CredentialKind::ConnectionPassphrase, "same-id").unwrap();
        let proxy = credential_account(CredentialKind::ProxyPassword, "same-id").unwrap();
        assert_ne!(password, passphrase);
        assert_ne!(password, proxy);
        assert_ne!(passphrase, proxy);
        assert_eq!(password, "v1:connection-password:same-id");
    }

    /// 空白标识不得进入系统凭据库
    #[test]
    fn rejects_blank_credential_id() {
        let error = credential_account(CredentialKind::ProxyPassword, "  ").unwrap_err();
        assert!(error.to_string().contains("代理密码标识不能为空"));
    }

    /// 凭据匹配参数必须按 camelCase 跨端反序列化
    #[test]
    fn deserializes_credential_match_from_camel_case_fields() {
        let change: CredentialMatch =
            serde_json::from_str(r#"{"kind":"proxyPassword","id":"proxy-id","value":"candidate"}"#)
                .unwrap();
        assert_eq!(change.kind, CredentialKind::ProxyPassword);
        assert_eq!(change.id, "proxy-id");
        assert_eq!(change.value, "candidate");
    }

    /// 比较接口不得成为连接密码或私钥口令的通用验证入口
    #[tokio::test]
    async fn rejects_non_proxy_credential_matches() {
        let error = CredentialManager::default()
            .match_many(vec![CredentialMatch {
                kind: CredentialKind::ConnectionPassword,
                id: "connection-id".to_string(),
                value: "candidate".to_string(),
            }])
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "系统凭据比较仅支持代理密码");
    }
}
