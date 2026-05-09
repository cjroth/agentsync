pub mod clone;
pub mod compact;
pub mod diff;
pub mod hub;
pub mod init;
pub mod key;
pub mod push_pull;
pub mod restore;
pub mod snapshot;
pub mod status;
pub mod watch;

use std::path::Path;

pub fn require_config(path: &Path) -> anyhow::Result<crate::config::ConfigFile> {
    let cfg = crate::config::read_or_default(path)?;
    if cfg.vault.id.is_none() {
        anyhow::bail!(
            "no vault configured at {} — run `agentsync init` or `agentsync clone` first",
            path.display()
        );
    }
    Ok(cfg)
}
