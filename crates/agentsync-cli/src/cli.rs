use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

pub use agentsync_core::DEFAULT_LISTEN_ADDR;

#[derive(Debug, Parser)]
#[command(name = "agentsync", version, about = "Real-time directory sync engine")]
pub struct Cli {
    /// Operate on the vault at this directory. Falls back to the
    /// `AGENTSYNC_CWD` env var, then the current working directory.
    /// Applies to every subcommand except `clone`, which takes its own
    /// destination directory.
    #[arg(long, global = true, env = "AGENTSYNC_CWD", default_value = ".")]
    pub cwd: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new vault in the working directory (`--cwd`).
    Init(InitArgs),
    /// Watch and sync the vault at `--cwd` (default operation).
    Watch(WatchArgs),
    /// Clone an existing vault into a local directory.
    Clone(CloneArgs),
    /// Print connection state and counts.
    Status,
    /// One-shot scan & push.
    Push,
    /// One-shot pull.
    Pull,
    /// Restore the vault to a wall-clock timestamp.
    #[command(name = "restore-at")]
    RestoreAt(RestoreAtArgs),
    /// Manage named recovery points.
    Snapshot(SnapshotArgs),
    /// Show changes between two points in history.
    Diff(DiffArgs),
    /// Run a compaction pass.
    Compact,
    /// Manage vault keys.
    Key(KeyArgs),
    /// Manage the pinned hub identity (`hub_pubkey`).
    Hub(HubArgs),
    /// Generate (or install) shell completions for tab-completing
    /// subcommands and flags. With `--install`, writes the script to the
    /// conventional location for the chosen shell instead of stdout.
    Completions(CompletionsArgs),
    /// Print version.
    Version,
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to emit completions for.
    pub shell: ShellKind,
    /// Write the completion script to this shell's conventional location
    /// instead of stdout. Prints the destination path on success along with
    /// any one-time `.zshrc`/`.bashrc` line you may need to add. Supported
    /// for `bash`, `zsh`, and `fish`; for `powershell` and `elvish` pipe
    /// stdout into your profile.
    #[arg(long)]
    pub install: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    #[value(alias = "powershell", alias = "pwsh")]
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
    },
    /// Clear the pinned hub identity. Next connect will trust whatever the
    /// hub presents.
    Forget,
    /// Show the currently pinned hub identity.
    Show,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long)]
    pub rendezvous: Option<String>,
    /// Display name for this vault. Defaults to the basename of the vault
    /// directory. Sent in the handshake so cloning peers can default the
    /// local directory name.
    #[arg(long)]
    pub name: Option<String>,
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
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// Bind a websocket listener on ADDR (acts as rendezvous). If the flag is
    /// passed without a value, defaults to `0.0.0.0:443` — privileged on
    /// Unix; see the README for `setcap` / launchd-socket-activation
    /// instructions, or pass an unprivileged port explicitly
    /// (`--listen 0.0.0.0:8443`).
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
    /// Extra public keys to merge into `authorized_keys` on startup. Accepts
    /// `ssh-ed25519 <base64> [comment]` entries separated by newlines or
    /// commas (commas are handy for single-line shell exports). Comments
    /// and blank lines are allowed. Falls back to the
    /// `AGENTSYNC_AUTHORIZED_KEYS` env var. Useful for bootstrapping a
    /// fresh server (e.g. from a Fly.io / Railway secret).
    #[arg(long, env = "AGENTSYNC_AUTHORIZED_KEYS")]
    pub authorized_keys: Option<String>,
}

#[derive(Debug, Args)]
pub struct CloneArgs {
    /// Rendezvous WebSocket URL of the remote vault (e.g. wss://host:port).
    pub remote_url: String,
    /// Local directory to clone into. Defaults to the remote vault's
    /// `name` (probed during the handshake) or, failing that, the URL
    /// hostname.
    pub local_path: Option<PathBuf>,
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
pub struct RestoreAtArgs {
    /// RFC3339 timestamp or epoch milliseconds.
    pub timestamp: String,
}

#[derive(Debug, Args)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    pub op: SnapshotOp,
}

#[derive(Debug, Subcommand)]
pub enum SnapshotOp {
    Create { label: String },
    List,
    Restore { label: String },
    Delete { label: String },
}

#[derive(Debug, Args)]
pub struct DiffArgs {
    pub from: String,
    pub to: Option<String>,
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
        /// Override the identity-secret path (default:
        /// `~/.agentsync/id_ed25519`).
        #[arg(long)]
        identity: Option<PathBuf>,
    },
    /// Print the local pubkey in `ssh-ed25519 ...` format, suitable for
    /// pasting into someone else's authorized_keys.
    Show,
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
///
/// Two ergonomics on top of plain clap parsing:
///   - Bare-path shortcut: `agentsync ./vault [extra]` is rewritten to
///     `--cwd ./vault [extra]` so the global `--cwd` carries the directory.
///     If `[extra]` doesn't include a subcommand, `watch` is inserted.
///   - Default subcommand: when no subcommand appears anywhere in argv (e.g.
///     `agentsync --listen 0.0.0.0:8443`), `watch` is prepended so the flag
///     parses as a watch flag.
pub fn resolve_args(raw: &[String]) -> Vec<String> {
    if raw.is_empty() {
        return vec!["watch".to_string()];
    }
    // Top-level help/version flags short-circuit in clap; pass through.
    if matches!(raw[0].as_str(), "--help" | "-h" | "--version" | "-V") {
        return raw.to_vec();
    }

    // Lift a leading bare path into --cwd. Only when the token isn't itself
    // a known subcommand — so `agentsync init` keeps meaning init even if a
    // file named `init` happens to exist.
    let mut head: Vec<String> = Vec::new();
    let mut tail_start = 0usize;
    if !raw[0].starts_with('-') && !KNOWN_SUBCOMMANDS.iter().any(|c| *c == raw[0]) {
        let p = std::path::Path::new(&raw[0]);
        let path_like = raw[0].starts_with('/')
            || raw[0].starts_with('.')
            || raw[0].starts_with('~')
            || p.exists();
        if path_like {
            head.push("--cwd".to_string());
            head.push(raw[0].clone());
            tail_start = 1;
        }
    }

    let tail = &raw[tail_start..];
    let mut out = head;
    if has_explicit_subcommand(tail) {
        out.extend(tail.iter().cloned());
    } else {
        out.push("watch".to_string());
        out.extend(tail.iter().cloned());
    }
    out
}

/// Walk `args` looking for an explicit subcommand token. Skips
/// `--cwd <value>` (and `--cwd=value`) and individual top-level flags.
/// Other globals (`--help`, `--version`) don't take values; subcommand-local
/// value-taking flags can't appear before a subcommand, so this approximation
/// is sufficient.
fn has_explicit_subcommand(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let t = &args[i];
        if t == "--cwd" {
            i += 2;
            continue;
        }
        if t.starts_with("--cwd=") {
            i += 1;
            continue;
        }
        if t.starts_with('-') {
            i += 1;
            continue;
        }
        return KNOWN_SUBCOMMANDS.iter().any(|c| *c == t);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
    fn dot_path_invokes_watch_with_cwd() {
        let raw = vec!["./vault".to_string()];
        let out = resolve_args(&raw);
        assert_eq!(out, vec!["--cwd", "./vault", "watch"]);
    }

    #[test]
    fn dot_path_with_subcommand_lifts_to_cwd() {
        let raw = vec!["./vault".to_string(), "status".to_string()];
        let out = resolve_args(&raw);
        assert_eq!(out, vec!["--cwd", "./vault", "status"]);
    }

    #[test]
    fn cwd_then_subcommand_preserved() {
        let raw = vec![
            "--cwd".to_string(),
            "/tmp/v".to_string(),
            "init".to_string(),
        ];
        let out = resolve_args(&raw);
        assert_eq!(out, vec!["--cwd", "/tmp/v", "init"]);
    }

    #[test]
    fn cwd_only_falls_through_to_watch() {
        let raw = vec!["--cwd".to_string(), "/tmp/v".to_string()];
        let out = resolve_args(&raw);
        assert_eq!(out, vec!["watch", "--cwd", "/tmp/v"]);
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

    #[test]
    fn cwd_global_default_is_dot() {
        let cli = Cli::try_parse_from(["agentsync", "status"]).unwrap();
        assert_eq!(cli.cwd, PathBuf::from("."));
    }

    #[test]
    fn cwd_accepted_before_subcommand() {
        let cli = Cli::try_parse_from(["agentsync", "--cwd", "/tmp/v", "status"]).unwrap();
        assert_eq!(cli.cwd, PathBuf::from("/tmp/v"));
    }

    #[test]
    fn cwd_accepted_after_subcommand() {
        let cli = Cli::try_parse_from(["agentsync", "status", "--cwd", "/tmp/v"]).unwrap();
        assert_eq!(cli.cwd, PathBuf::from("/tmp/v"));
    }
}
