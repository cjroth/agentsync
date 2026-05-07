use anyhow::Result;
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

    let parsed = match cli::Cli::try_parse_from(
        std::iter::once("agentsync".to_string()).chain(resolved),
    ) {
        Ok(p) => p,
        Err(e) => {
            // clap returns DisplayHelp / DisplayVersion as Err with ExitCode::SUCCESS
            // semantics; print and exit cleanly without anyhow's "Error:" prefix.
            e.exit();
        }
    };

    match parsed.command {
        cli::Command::Init(args) => commands::init::run(args).await,
        cli::Command::Watch(args) => commands::watch::run(args).await,
        cli::Command::Clone(args) => commands::clone::run(args).await,
        cli::Command::Status(args) => commands::status::run(args).await,
        cli::Command::Push(args) => commands::push_pull::run_push(args).await,
        cli::Command::Pull(args) => commands::push_pull::run_pull(args).await,
        cli::Command::RestoreAt(args) => commands::restore::run_restore_at(args).await,
        cli::Command::Snapshot(args) => commands::snapshot::run(args).await,
        cli::Command::Diff(args) => commands::diff::run(args).await,
        cli::Command::Compact(args) => commands::compact::run(args).await,
        cli::Command::Key(args) => commands::key::run(args).await,
        cli::Command::Hub(args) => commands::hub::run(args).await,
        cli::Command::Completions(args) => {
            let shell = match args.shell {
                cli::ShellKind::Bash => Shell::Bash,
                cli::ShellKind::Zsh => Shell::Zsh,
                cli::ShellKind::Fish => Shell::Fish,
                cli::ShellKind::PowerShell => Shell::PowerShell,
                cli::ShellKind::Elvish => Shell::Elvish,
            };
            let mut cmd = cli::Cli::command();
            clap_complete::generate(shell, &mut cmd, "agentsync", &mut std::io::stdout());
            Ok(())
        }
        cli::Command::Version => {
            println!("agentsync {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("AGENTSYNC_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
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
