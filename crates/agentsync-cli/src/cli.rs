use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "agentsync", version, about = "Real-time directory sync engine")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new vault in the current directory.
    Init(InitArgs),
    /// Watch and sync a directory (default operation).
    Watch(WatchArgs),
    /// Clone an existing vault into a local directory.
    Clone(CloneArgs),
    /// Print connection state and counts.
    Status(StatusArgs),
    /// One-shot scan & push.
    Push(PushPullArgs),
    /// One-shot pull.
    Pull(PushPullArgs),
    /// Restore the vault to a wall-clock timestamp.
    #[command(name = "restore-at")]
    RestoreAt(RestoreAtArgs),
    /// Manage named recovery points.
    Snapshot(SnapshotArgs),
    /// Show changes between two points in history.
    Diff(DiffArgs),
    /// Run a compaction pass.
    Compact(CompactArgs),
    /// Manage vault keys.
    Key(KeyArgs),
    /// Print version.
    Version,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long)]
    pub rendezvous: Option<String>,
    #[arg(long)]
    pub key_source: Option<String>,
    #[arg(long, default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    pub path: Option<PathBuf>,
    /// Bind a websocket listener on ADDR (acts as rendezvous).
    #[arg(long)]
    pub listen: Option<String>,
    /// Override the rendezvous URL configured in config.toml.
    #[arg(long)]
    pub rendezvous: Option<String>,
    /// Don't connect to any rendezvous.
    #[arg(long)]
    pub offline: bool,
}

#[derive(Debug, Args)]
pub struct CloneArgs {
    /// Local directory to clone into.
    pub local_path: PathBuf,
    /// Rendezvous WebSocket URL (e.g. ws://host:port).
    #[arg(long)]
    pub rendezvous: String,
    /// Vault key (base64). Omit to read from $AGENTSYNC_KEY.
    #[arg(long)]
    pub key: Option<String>,
    /// Optional vault id. If omitted, discovered from the server during
    /// the handshake — typo-safety for users who already know it.
    #[arg(long)]
    pub vault_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct PushPullArgs {
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct RestoreAtArgs {
    /// RFC3339 timestamp or epoch milliseconds.
    pub timestamp: String,
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    pub op: SnapshotOp,
}

#[derive(Debug, Subcommand)]
pub enum SnapshotOp {
    Create {
        label: String,
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    List {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    Restore {
        label: String,
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    Delete {
        label: String,
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

#[derive(Debug, Args)]
pub struct DiffArgs {
    pub from: String,
    pub to: Option<String>,
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct CompactArgs {
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct KeyArgs {
    #[command(subcommand)]
    pub op: KeyOp,
}

#[derive(Debug, Subcommand)]
pub enum KeyOp {
    /// Generate a fresh vault key (does not persist anywhere).
    Generate,
    /// Show the key configured for this vault.
    Show {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Persist the current vault key to a keyring/file.
    Store {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

const KNOWN_SUBCOMMANDS: &[&str] = &[
    "init",
    "watch",
    "clone",
    "status",
    "push",
    "pull",
    "restore-at",
    "snapshot",
    "diff",
    "compact",
    "key",
    "version",
    "help",
];

/// Implements the spec's argument resolution rules. Returns the actual argv
/// (sans program name) that should be fed to the clap parser.
pub fn resolve_args(raw: &[String]) -> Vec<String> {
    if raw.is_empty() {
        return vec!["watch".to_string()];
    }
    // Top-level help/version flags should pass straight through to clap.
    if matches!(raw[0].as_str(), "--help" | "-h" | "--version" | "-V") {
        return raw.to_vec();
    }
    if raw[0].starts_with("--") || raw[0].starts_with('-') {
        // First arg is a flag → run watch with these flags.
        let mut out = vec!["watch".to_string()];
        out.extend(raw.iter().cloned());
        return out;
    }
    if KNOWN_SUBCOMMANDS.iter().any(|c| *c == raw[0]) {
        return raw.to_vec();
    }
    // First arg looks path-like → watch <path> [flags...]
    if raw[0].starts_with('/') || raw[0].starts_with('.') || raw[0].starts_with('~') {
        let mut out = vec!["watch".to_string()];
        out.extend(raw.iter().cloned());
        return out;
    }
    let p = std::path::Path::new(&raw[0]);
    if p.exists() {
        let mut out = vec!["watch".to_string()];
        out.extend(raw.iter().cloned());
        return out;
    }
    // Fallthrough: treat as unknown subcommand and let clap error.
    raw.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_runs_watch() {
        assert_eq!(resolve_args(&[]), vec!["watch"]);
    }

    #[test]
    fn known_subcommand_passthrough() {
        let raw = vec!["init".to_string()];
        assert_eq!(resolve_args(&raw), raw);
    }

    #[test]
    fn flag_invokes_watch() {
        let raw = vec!["--listen".to_string(), "0.0.0.0:8443".to_string()];
        let out = resolve_args(&raw);
        assert_eq!(out[0], "watch");
        assert_eq!(out[1], "--listen");
    }

    #[test]
    fn dot_path_invokes_watch() {
        let raw = vec!["./vault".to_string()];
        assert_eq!(resolve_args(&raw)[0], "watch");
    }
}
