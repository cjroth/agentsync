use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

pub use agentsync_core::DEFAULT_LISTEN_ADDR;

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
    /// Manage the pinned hub identity (`hub_pubkey`).
    Hub(HubArgs),
    /// Generate shell completions for tab-completing subcommands and flags.
    /// Pipe the output into your shell's completions directory, e.g.
    /// `agentsync completions bash > /etc/bash_completion.d/agentsync`.
    Completions(CompletionsArgs),
    /// Print version.
    Version,
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to emit completions for.
    pub shell: ShellKind,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Elvish,
}

#[derive(Debug, Args)]
pub struct HubArgs {
    #[command(subcommand)]
    pub op: HubOp,
}

#[derive(Debug, Subcommand)]
pub enum HubOp {
    /// Pin (or replace) the hub identity used for this vault.
    Trust {
        /// Pubkey in `ssh-ed25519 <base64>` form.
        pubkey: String,
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Clear the pinned hub identity. Next connect will trust whatever the
    /// hub presents.
    Forget {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show the currently pinned hub identity.
    Show {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long)]
    pub rendezvous: Option<String>,
    /// Override the identity-secret path. Defaults to
    /// `~/.agentsync/id_ed25519` (shared across vaults, like
    /// `~/.ssh/id_ed25519`).
    #[arg(long)]
    pub identity: Option<PathBuf>,
    /// Skip creating / updating `.gitignore` and `.agentsignore`. By default
    /// `init` ensures both contain `.agentsync/` so the per-vault state
    /// directory isn't accidentally committed or re-synced.
    #[arg(long)]
    pub no_ignore_files: bool,
    #[arg(long, default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    pub path: Option<PathBuf>,
    /// Bind a websocket listener on ADDR (acts as rendezvous). If the flag is
    /// passed without a value, defaults to `0.0.0.0:1234`.
    #[arg(long, num_args = 0..=1, default_missing_value = DEFAULT_LISTEN_ADDR)]
    pub listen: Option<String>,
    /// Override the rendezvous URL configured in config.toml.
    #[arg(long)]
    pub rendezvous: Option<String>,
    /// Don't connect to any rendezvous.
    #[arg(long)]
    pub offline: bool,
    /// Override the ssh-agent socket path. Falls back to
    /// `[identity] agent_socket` and then `$SSH_AUTH_SOCK`. Only used when
    /// the identity backend is `agent`.
    #[arg(long)]
    pub identity_agent: Option<PathBuf>,
    /// Select an ssh-agent identity by pubkey (in `ssh-ed25519 <base64>`
    /// form, or path to a `.pub` file). Setting this switches the identity
    /// backend to `agent`.
    #[arg(long)]
    pub identity_agent_pubkey: Option<String>,
}

#[derive(Debug, Args)]
pub struct CloneArgs {
    /// Local directory to clone into.
    pub local_path: PathBuf,
    /// Rendezvous WebSocket URL (e.g. ws://host:port).
    #[arg(long)]
    pub rendezvous: String,
    /// Path for the local identity secret. Defaults to
    /// `~/.agentsync/id_ed25519` (shared across vaults). If the file
    /// already exists, it's reused; otherwise a fresh ed25519 keypair is
    /// generated.
    #[arg(long)]
    pub identity: Option<PathBuf>,
    /// Optional vault id. If omitted, discovered from the server during
    /// the handshake — typo-safety for users who already know it.
    #[arg(long)]
    pub vault_id: Option<String>,
    /// Pre-pin the hub's identity pubkey, skipping the interactive trust
    /// prompt. Suitable for CI / scripted setups.
    #[arg(long)]
    pub accept_hub_key: Option<String>,
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
    /// Generate a fresh ed25519 identity for this vault. Refuses to overwrite
    /// an existing identity file.
    Generate {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Override the identity-secret path (default:
        /// `~/.agentsync/id_ed25519`).
        #[arg(long)]
        identity: Option<PathBuf>,
    },
    /// Print the local pubkey in `ssh-ed25519 ...` format, suitable for
    /// pasting into someone else's authorized_keys.
    Show {
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
    "hub",
    "completions",
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

    #[test]
    fn listen_without_value_uses_default_addr() {
        let cli = Cli::try_parse_from(["agentsync", "watch", "--listen"]).unwrap();
        let listen = match cli.command {
            Command::Watch(args) => args.listen,
            _ => panic!("expected watch"),
        };
        assert_eq!(listen.as_deref(), Some(DEFAULT_LISTEN_ADDR));
    }

    #[test]
    fn listen_with_explicit_value_keeps_it() {
        let cli =
            Cli::try_parse_from(["agentsync", "watch", "--listen", "127.0.0.1:9999"]).unwrap();
        let listen = match cli.command {
            Command::Watch(args) => args.listen,
            _ => panic!("expected watch"),
        };
        assert_eq!(listen.as_deref(), Some("127.0.0.1:9999"));
    }

    #[test]
    fn no_listen_flag_keeps_listen_none() {
        let cli = Cli::try_parse_from(["agentsync", "watch"]).unwrap();
        let listen = match cli.command {
            Command::Watch(args) => args.listen,
            _ => panic!("expected watch"),
        };
        assert!(listen.is_none());
    }
}
