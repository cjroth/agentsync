use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub vault: VaultSection,
    #[serde(default)]
    pub key: KeySection,
    #[serde(default)]
    pub sync: SyncSection,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct VaultSection {
    pub id: Option<String>,
    pub rendezvous_url: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct KeySection {
    /// "keyring" | "env" | "file"
    pub source: Option<String>,
    pub keyring_name: Option<String>,
    /// Inline base64 key, only used when source = "file".
    pub key_b64: Option<String>,
    pub env_var: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSection {
    /// File extensions (without the dot) to sync. Defaults to markdown only.
    /// Each extension here generates an `**/*.<ext>` include pattern and is
    /// ingested as Automerge text.
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
    /// Extra glob patterns to include beyond what `extensions` produces.
    /// If non-empty, these are appended to the derived include list.
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default = "default_attachment_max")]
    pub attachment_max_bytes: u64,
    #[serde(default = "default_text_max")]
    pub text_file_max_bytes: u64,
    #[serde(default = "default_retention")]
    pub log_retention_days: u32,
}

fn default_extensions() -> Vec<String> {
    vec!["md".into(), "markdown".into()]
}
fn default_exclude() -> Vec<String> {
    vec![
        "**/.git/**".into(),
        "**/node_modules/**".into(),
        "**/.DS_Store".into(),
        "**/.agentsync/**".into(),
    ]
}
fn default_attachment_max() -> u64 {
    10 * 1024 * 1024
}
fn default_text_max() -> u64 {
    1 * 1024 * 1024
}
fn default_retention() -> u32 {
    30
}

impl Default for SyncSection {
    fn default() -> Self {
        Self {
            extensions: default_extensions(),
            exclude: default_exclude(),
            include: Vec::new(),
            attachment_max_bytes: default_attachment_max(),
            text_file_max_bytes: default_text_max(),
            log_retention_days: default_retention(),
        }
    }
}

impl SyncSection {
    /// Resolve to an `agentsync_core::BindOptions` honoring `extensions`,
    /// `include`, and `exclude`.
    pub fn to_bind_options(&self) -> agentsync_core::BindOptions {
        let mut include: Vec<String> = self
            .extensions
            .iter()
            .map(|e| format!("**/*.{}", e.trim_start_matches('.').to_ascii_lowercase()))
            .collect();
        for extra in &self.include {
            if !include.contains(extra) {
                include.push(extra.clone());
            }
        }
        let text_extensions = self
            .extensions
            .iter()
            .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
            .collect();
        agentsync_core::BindOptions {
            exclude_patterns: self.exclude.clone(),
            include_patterns: include,
            text_extensions,
            attachment_max_bytes: self.attachment_max_bytes,
            text_file_max_bytes: self.text_file_max_bytes,
        }
    }
}

pub fn config_path(vault_root: &Path) -> PathBuf {
    vault_root.join(".agentsync").join("config.toml")
}

pub fn read_or_default(vault_root: &Path) -> Result<ConfigFile> {
    let path = config_path(vault_root);
    if !path.exists() {
        return Ok(ConfigFile::default());
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let s = std::str::from_utf8(&bytes).context("config.toml not valid utf-8")?;
    let cfg: ConfigFile = toml::from_str(s)
        .with_context(|| format!("parse {}", path.display()))?;
    Ok(cfg)
}

pub fn write(vault_root: &Path, cfg: &ConfigFile) -> Result<()> {
    let dir = vault_root.join(".agentsync");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let bytes = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, bytes)?;
    Ok(())
}

pub fn resolve_key(cfg: &ConfigFile, env_override: Option<&str>) -> Result<[u8; 32]> {
    if let Some(b64) = env_override {
        return agentsync_core::decode_key(b64).map_err(|e| anyhow::anyhow!(e));
    }
    let source = cfg.key.source.as_deref().unwrap_or("env");
    match source {
        "env" => {
            let var = cfg.key.env_var.as_deref().unwrap_or("AGENTSYNC_KEY");
            let v = std::env::var(var)
                .with_context(|| format!("env var {} not set", var))?;
            agentsync_core::decode_key(&v).map_err(|e| anyhow::anyhow!(e))
        }
        "file" => {
            let b64 = cfg
                .key
                .key_b64
                .as_deref()
                .context("config.toml [key] missing key_b64 for file source")?;
            agentsync_core::decode_key(b64).map_err(|e| anyhow::anyhow!(e))
        }
        "keyring" => {
            // Keyring backend is out of scope for v1; users can still drop to env.
            let var = cfg.key.env_var.as_deref().unwrap_or("AGENTSYNC_KEY");
            let v = std::env::var(var).with_context(|| {
                format!(
                    "keyring source not yet implemented; set {} as fallback",
                    var
                )
            })?;
            agentsync_core::decode_key(&v).map_err(|e| anyhow::anyhow!(e))
        }
        other => Err(anyhow::anyhow!(
            "unknown key source: {}, expected env|file|keyring",
            other
        )),
    }
}

