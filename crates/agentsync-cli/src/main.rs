use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use std::path::PathBuf;

mod cli;
mod commands;
mod config;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    // Custom argument resolution per spec section "Argument resolution rules".
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let resolved = cli::resolve_args(&raw);

    let parsed =
        match cli::Cli::try_parse_from(std::iter::once("agentsync".to_string()).chain(resolved)) {
            Ok(p) => p,
            Err(e) => {
                // clap returns DisplayHelp / DisplayVersion as Err with ExitCode::SUCCESS
                // semantics; print and exit cleanly without anyhow's "Error:" prefix.
                e.exit();
            }
        };

    let cwd = parsed.cwd;
    match parsed.command {
        cli::Command::Init(args) => commands::init::run(cwd, args).await,
        cli::Command::Watch(args) => commands::watch::run(cwd, args).await,
        cli::Command::Clone(args) => commands::clone::run(args).await,
        cli::Command::Status => commands::status::run(cwd).await,
        cli::Command::Push => commands::push_pull::run_push(cwd).await,
        cli::Command::Pull => commands::push_pull::run_pull(cwd).await,
        cli::Command::RestoreAt(args) => commands::restore::run_restore_at(cwd, args).await,
        cli::Command::Snapshot(args) => commands::snapshot::run(cwd, args).await,
        cli::Command::Diff(args) => commands::diff::run(cwd, args).await,
        cli::Command::Compact => commands::compact::run(cwd).await,
        cli::Command::Key(args) => commands::key::run(cwd, args).await,
        cli::Command::Hub(args) => commands::hub::run(cwd, args).await,
        cli::Command::Completions(args) => run_completions(args),
        cli::Command::Version => {
            println!("agentsync {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

fn run_completions(args: cli::CompletionsArgs) -> Result<()> {
    let shell = clap_shell(args.shell);
    if !args.install {
        let mut cmd = cli::Cli::command();
        clap_complete::generate(shell, &mut cmd, "agentsync", &mut std::io::stdout());
        return Ok(());
    }
    install_completions(args.shell, shell)
}

fn clap_shell(s: cli::ShellKind) -> Shell {
    match s {
        cli::ShellKind::Bash => Shell::Bash,
        cli::ShellKind::Zsh => Shell::Zsh,
        cli::ShellKind::Fish => Shell::Fish,
        cli::ShellKind::PowerShell => Shell::PowerShell,
        cli::ShellKind::Elvish => Shell::Elvish,
    }
}

/// Write the completion script to the conventional per-shell location and
/// print the path (plus any followup line the user must add to their rc
/// file). Bash/zsh/fish have well-defined dropbox paths; powershell and
/// elvish don't, so `--install` for those refuses with a hint.
fn install_completions(kind: cli::ShellKind, shell: Shell) -> Result<()> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("$HOME (or %USERPROFILE%) is not set")?;

    let (path, hint): (PathBuf, Option<&'static str>) = match kind {
        cli::ShellKind::Bash => (
            home.join(".local/share/bash-completion/completions/agentsync"),
            None,
        ),
        cli::ShellKind::Zsh => (
            home.join(".zfunc/_agentsync"),
            Some(
                "Make sure ~/.zfunc is on $fpath and `compinit` is loaded in ~/.zshrc:\n\
                 \tfpath=(~/.zfunc $fpath)\n\
                 \tautoload -U compinit && compinit",
            ),
        ),
        cli::ShellKind::Fish => (home.join(".config/fish/completions/agentsync.fish"), None),
        cli::ShellKind::PowerShell | cli::ShellKind::Elvish => {
            anyhow::bail!(
                "--install isn't supported for {:?} (no conventional dropbox path); \
                 pipe stdout into your shell profile instead",
                kind
            );
        }
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut cmd = cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "agentsync", &mut buf);
    std::fs::write(&path, &buf).with_context(|| format!("write {}", path.display()))?;
    println!("installed completions to {}", path.display());
    if let Some(h) = hint {
        println!("{}", h);
    }
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_env("AGENTSYNC_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // Logs go to stderr so they don't pollute stdout-based protocols (the
    // listen-port handshake the harness reads from stdout, etc).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

pub fn default_storage_path(working_dir: &PathBuf) -> PathBuf {
    working_dir.join(".agentsync")
}
