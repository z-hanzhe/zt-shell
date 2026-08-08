//! SSH 服务端主机密钥的持久化与校验

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use russh::keys::ssh_key::{HashAlg, PublicKey};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::types::{HostKeyApproval, HostKeyChallenge, HostKeyConfirmationKind};

/// 当前主机密钥文件格式版本
const KNOWN_HOSTS_VERSION: u8 = 1;

/// 主机密钥校验结果
pub enum HostKeyVerification {
    /// 服务端密钥可信，可以继续认证
    Trusted,
    /// 需要先由用户确认服务端密钥
    ConfirmationRequired(HostKeyChallenge),
}

/// 应用私有的主机密钥存储
#[derive(Clone)]
pub struct HostKeyStore {
    inner: Arc<HostKeyStoreInner>,
}

/// 主机密钥存储共享状态
struct HostKeyStoreInner {
    path: PathBuf,
    access: Mutex<()>,
}

/// 主机密钥文件
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnownHostsFile {
    #[serde(default = "known_hosts_version")]
    version: u8,
    #[serde(default)]
    hosts: Vec<KnownHostEntry>,
}

/// 一条按目标主机与端口绑定的可信密钥
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnownHostEntry {
    host: String,
    port: u16,
    algorithm: String,
    fingerprint: String,
    public_key: String,
}

impl Default for KnownHostsFile {
    fn default() -> Self {
        Self {
            version: KNOWN_HOSTS_VERSION,
            hosts: Vec::new(),
        }
    }
}

impl HostKeyStore {
    /// 使用指定文件创建应用主机密钥存储
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: Arc::new(HostKeyStoreInner {
                path,
                access: Mutex::new(()),
            }),
        }
    }

    /// 校验服务端密钥，并仅在授权与本次完整公钥精确一致时保存或替换记录
    pub async fn verify(
        &self,
        host: &str,
        port: u16,
        server_public_key: &PublicKey,
        approval: Option<&HostKeyApproval>,
    ) -> Result<HostKeyVerification> {
        let normalized_host = normalize_host(host);
        let public_key = server_public_key
            .to_openssh()
            .context("生成服务端主机公钥失败")?;
        let algorithm = server_public_key.algorithm().to_string();
        let fingerprint = server_public_key.fingerprint(HashAlg::Sha256).to_string();
        let _guard = self.inner.access.lock().await;
        let mut file = load_known_hosts(&self.inner.path).await?;
        let existing_index = file
            .hosts
            .iter()
            .position(|entry| entry.host == normalized_host && entry.port == port);

        if let Some(index) = existing_index {
            let existing = &file.hosts[index];
            if existing.public_key == public_key {
                return Ok(HostKeyVerification::Trusted);
            }

            let challenge = HostKeyChallenge {
                kind: HostKeyConfirmationKind::Changed,
                host: host.to_string(),
                port,
                algorithm: algorithm.clone(),
                fingerprint: fingerprint.clone(),
                known_fingerprint: Some(existing.fingerprint.clone()),
                public_key: public_key.clone(),
            };
            if approval_matches(approval, &public_key, true) {
                file.hosts[index] = KnownHostEntry {
                    host: normalized_host,
                    port,
                    algorithm,
                    fingerprint,
                    public_key,
                };
                save_known_hosts(&self.inner.path, &file).await?;
                return Ok(HostKeyVerification::Trusted);
            }
            return Ok(HostKeyVerification::ConfirmationRequired(challenge));
        }

        let challenge = HostKeyChallenge {
            kind: HostKeyConfirmationKind::Unknown,
            host: host.to_string(),
            port,
            algorithm: algorithm.clone(),
            fingerprint: fingerprint.clone(),
            known_fingerprint: None,
            public_key: public_key.clone(),
        };
        if approval_matches(approval, &public_key, false) {
            file.hosts.push(KnownHostEntry {
                host: normalized_host,
                port,
                algorithm,
                fingerprint,
                public_key,
            });
            save_known_hosts(&self.inner.path, &file).await?;
            return Ok(HostKeyVerification::Trusted);
        }
        Ok(HostKeyVerification::ConfirmationRequired(challenge))
    }
}

/// 返回主机密钥文件的当前格式版本
fn known_hosts_version() -> u8 {
    KNOWN_HOSTS_VERSION
}

/// 统一主机比较形式，避免域名或 IPv6 字母大小写导致重复确认
fn normalize_host(host: &str) -> String {
    let trimmed = host.trim();
    let without_brackets = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);
    without_brackets.to_ascii_lowercase()
}

/// 判断用户授权是否精确对应本次公钥与更新场景
fn approval_matches(
    approval: Option<&HostKeyApproval>,
    public_key: &str,
    replace_existing: bool,
) -> bool {
    approval.is_some_and(|value| {
        value.public_key == public_key && value.replace_existing == replace_existing
    })
}

/// 从磁盘读取主机密钥文件；文件不存在时返回空记录
async fn load_known_hosts(path: &Path) -> Result<KnownHostsFile> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return load_interrupted_write(path).await;
        }
        Err(error) => {
            return Err(error).context("读取主机密钥记录失败");
        }
    };
    parse_known_hosts(&bytes)
}

/// 主文件缺失时尝试恢复一次未完成替换留下的临时文件
async fn load_interrupted_write(path: &Path) -> Result<KnownHostsFile> {
    let temporary_path = temporary_path(path);
    let bytes = match tokio::fs::read(&temporary_path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(KnownHostsFile::default()),
        Err(error) => return Err(error).context("恢复主机密钥记录失败"),
    };
    let file = parse_known_hosts(&bytes)?;
    replace_file(&temporary_path, path)
        .await
        .context("恢复主机密钥记录失败")?;
    Ok(file)
}

/// 解析并校验主机密钥文件版本
fn parse_known_hosts(bytes: &[u8]) -> Result<KnownHostsFile> {
    let file: KnownHostsFile =
        serde_json::from_slice(bytes).context("主机密钥记录已损坏，已阻止连接")?;
    if file.version != KNOWN_HOSTS_VERSION {
        return Err(anyhow!("不支持的主机密钥记录版本：{}", file.version));
    }
    Ok(file)
}

/// 将主机密钥文件写入同目录临时文件后替换，避免并发写入产生半条记录
async fn save_known_hosts(path: &Path, file: &KnownHostsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("创建主机密钥存储目录失败")?;
    }
    let bytes = serde_json::to_vec_pretty(file).context("生成主机密钥记录失败")?;
    let temporary_path = temporary_path(path);
    tokio::fs::write(&temporary_path, bytes)
        .await
        .context("写入主机密钥临时记录失败")?;
    replace_file(&temporary_path, path)
        .await
        .context("保存主机密钥记录失败")
}

/// 返回与主机密钥文件同目录的临时文件路径
fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

/// 跨平台替换目标文件；Windows 不允许 rename 直接覆盖时先移除旧文件
async fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    match tokio::fs::rename(source, target).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            tokio::fs::remove_file(target).await?;
            tokio::fs::rename(source, target).await
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    /// 测试用 Ed25519 公钥一
    const TEST_KEY_ONE: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    /// 测试用 Ed25519 公钥二
    const TEST_KEY_TWO: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";

    /// 创建互不冲突的测试存储路径
    fn test_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("zt-shell-host-keys-{}", Uuid::new_v4()))
            .join("known_hosts.json")
    }

    /// 解析一把测试服务端公钥
    fn test_public_key(encoded: &str) -> PublicKey {
        russh::keys::parse_public_key_base64(encoded).expect("测试公钥应解析成功")
    }

    /// 首次确认后应持久化，后续相同密钥应直接通过
    #[tokio::test]
    async fn trusts_approved_key_and_reuses_record() {
        let path = test_path();
        let store = HostKeyStore::new(path.clone());
        let key = test_public_key(TEST_KEY_ONE);
        let HostKeyVerification::ConfirmationRequired(challenge) = store
            .verify("Example.COM", 22, &key, None)
            .await
            .expect("首次校验应成功")
        else {
            panic!("首次连接应要求确认");
        };
        assert_eq!(challenge.kind, HostKeyConfirmationKind::Unknown);

        let approval = HostKeyApproval {
            public_key: challenge.public_key,
            replace_existing: false,
        };
        assert!(matches!(
            store.verify("example.com", 22, &key, Some(&approval)).await,
            Ok(HostKeyVerification::Trusted)
        ));

        let reloaded = HostKeyStore::new(path.clone());
        assert!(matches!(
            reloaded.verify("EXAMPLE.COM", 22, &key, None).await,
            Ok(HostKeyVerification::Trusted)
        ));
        let _ = tokio::fs::remove_dir_all(path.parent().expect("测试路径应有父目录")).await;
    }

    /// 授权公钥与第二次握手不一致时不得保存或放行
    #[tokio::test]
    async fn rejects_stale_approval() {
        let path = test_path();
        let store = HostKeyStore::new(path.clone());
        let first_key = test_public_key(TEST_KEY_ONE);
        let second_key = test_public_key(TEST_KEY_TWO);
        let HostKeyVerification::ConfirmationRequired(first_challenge) = store
            .verify("server", 2222, &first_key, None)
            .await
            .expect("首次校验应成功")
        else {
            panic!("首次连接应要求确认");
        };
        let approval = HostKeyApproval {
            public_key: first_challenge.public_key,
            replace_existing: false,
        };

        let HostKeyVerification::ConfirmationRequired(second_challenge) = store
            .verify("server", 2222, &second_key, Some(&approval))
            .await
            .expect("二次校验应成功")
        else {
            panic!("变化后的公钥不得使用旧授权放行");
        };
        assert_eq!(second_challenge.kind, HostKeyConfirmationKind::Unknown);
        assert!(!path.exists());
        let _ = tokio::fs::remove_dir_all(path.parent().expect("测试路径应有父目录")).await;
    }

    /// 已知主机密钥变化时必须明确更新，并替换旧记录
    #[tokio::test]
    async fn requires_explicit_replacement_for_changed_key() {
        let path = test_path();
        let store = HostKeyStore::new(path.clone());
        let old_key = test_public_key(TEST_KEY_ONE);
        let new_key = test_public_key(TEST_KEY_TWO);
        let HostKeyVerification::ConfirmationRequired(initial) = store
            .verify("server", 22, &old_key, None)
            .await
            .expect("首次校验应成功")
        else {
            panic!("首次连接应要求确认");
        };
        store
            .verify(
                "server",
                22,
                &old_key,
                Some(&HostKeyApproval {
                    public_key: initial.public_key,
                    replace_existing: false,
                }),
            )
            .await
            .expect("首次授权应保存成功");

        let HostKeyVerification::ConfirmationRequired(changed) = store
            .verify("server", 22, &new_key, None)
            .await
            .expect("变化校验应成功")
        else {
            panic!("密钥变化应要求确认");
        };
        assert_eq!(changed.kind, HostKeyConfirmationKind::Changed);
        assert!(changed.known_fingerprint.is_some());

        let wrong_mode = store
            .verify(
                "server",
                22,
                &new_key,
                Some(&HostKeyApproval {
                    public_key: changed.public_key.clone(),
                    replace_existing: false,
                }),
            )
            .await
            .expect("错误更新方式应返回确认信息");
        assert!(matches!(
            wrong_mode,
            HostKeyVerification::ConfirmationRequired(_)
        ));

        assert!(matches!(
            store
                .verify(
                    "server",
                    22,
                    &new_key,
                    Some(&HostKeyApproval {
                        public_key: changed.public_key,
                        replace_existing: true,
                    }),
                )
                .await,
            Ok(HostKeyVerification::Trusted)
        ));
        assert!(matches!(
            store.verify("server", 22, &new_key, None).await,
            Ok(HostKeyVerification::Trusted)
        ));
        let _ = tokio::fs::remove_dir_all(path.parent().expect("测试路径应有父目录")).await;
    }

    /// 相同主机的不同端口必须分别建立可信记录
    #[tokio::test]
    async fn separates_records_by_port() {
        let path = test_path();
        let store = HostKeyStore::new(path.clone());
        let key = test_public_key(TEST_KEY_ONE);
        let HostKeyVerification::ConfirmationRequired(challenge) = store
            .verify("server", 22, &key, None)
            .await
            .expect("首次校验应成功")
        else {
            panic!("首次连接应要求确认");
        };
        store
            .verify(
                "server",
                22,
                &key,
                Some(&HostKeyApproval {
                    public_key: challenge.public_key,
                    replace_existing: false,
                }),
            )
            .await
            .expect("首次授权应保存成功");

        let HostKeyVerification::ConfirmationRequired(other_port) = store
            .verify("server", 2222, &key, None)
            .await
            .expect("其他端口校验应成功")
        else {
            panic!("其他端口应独立要求确认");
        };
        assert_eq!(other_port.kind, HostKeyConfirmationKind::Unknown);
        let _ = tokio::fs::remove_dir_all(path.parent().expect("测试路径应有父目录")).await;
    }

    /// 主机密钥文件损坏时必须阻止连接，不得退回空记录
    #[tokio::test]
    async fn fails_closed_for_corrupted_store() {
        let path = test_path();
        tokio::fs::create_dir_all(path.parent().expect("测试路径应有父目录"))
            .await
            .expect("测试目录应创建成功");
        tokio::fs::write(&path, b"not-json")
            .await
            .expect("损坏记录应写入成功");
        let store = HostKeyStore::new(path.clone());
        let error = match store
            .verify("server", 22, &test_public_key(TEST_KEY_ONE), None)
            .await
        {
            Ok(_) => panic!("损坏记录必须阻止连接"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("主机密钥记录已损坏"));
        let _ = tokio::fs::remove_dir_all(path.parent().expect("测试路径应有父目录")).await;
    }
}
