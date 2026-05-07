use agentsync_core::{encode_key, generate_vault_key, CreateOptions, Vault, VaultKey};
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::Instant;

/// One end-to-end scenario: a rendezvous peer + zero-or-more client peers,
/// each running the real `agentsync` binary in its own temp directory.
pub struct E2EVault {
    binary: PathBuf,
    pub vault_id: String,
    pub vault_key_b64: String,
    pub rendezvous_url: String,
    pub rendezvous: Peer,
    pub peers: Vec<Peer>,
}

/// One running `agentsync` process bound to a temp directory. `proc` is
/// `None` when the peer has been killed (or hasn't been spawned yet) but the
/// storage dir is being preserved for a later restart.
pub struct Peer {
    pub name: String,
    pub dir: TempDir,
    proc: Option<Child>,
}

impl E2EVault {
    /// Spin up a brand-new vault with a single listening peer named `rendezvous`.
    pub async fn new() -> Result<Self> {
        let binary = locate_binary()?;

        // Bootstrap the rendezvous storage in-process so we know the vault_id
        // and key up front. The CLI we spawn afterwards loads this state.
        let rendezvous_dir = TempDir::new()?;
        let storage = rendezvous_dir.path().join(".agentsync");
        let vault_key: VaultKey = generate_vault_key();
        let (vault, created) = Vault::create(CreateOptions {
            rendezvous_url: None,
            vault_key: Some(vault_key),
            storage_path: storage.clone(),
        })
        .await?;
        vault.flush().await?;
        drop(vault); // release any background tasks holding the storage

        let vault_id = created.vault_id;
        let vault_key_b64 = encode_key(&created.vault_key);

        write_config(
            rendezvous_dir.path(),
            &vault_id,
            None,
        )?;

        // Bind to a random port. We'll parse the actual port from stdout.
        let mut cmd = base_command(&binary, rendezvous_dir.path(), &vault_key_b64);
        cmd.arg("--listen").arg("127.0.0.1:0");
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn rendezvous")?;

        let port = read_listen_port(&mut child)
            .await
            .context("waiting for rendezvous to bind")?;
        let rendezvous_url = format!("ws://127.0.0.1:{}", port);

        // Drain remaining stdout/stderr so the pipes don't fill.
        spawn_log_drainer(&mut child, "rendezvous");

        let rendezvous = Peer {
            name: "rendezvous".into(),
            dir: rendezvous_dir,
            proc: Some(child),
        };

        Ok(E2EVault {
            binary,
            vault_id,
            vault_key_b64,
            rendezvous_url,
            rendezvous,
            peers: Vec::new(),
        })
    }

    /// Add a new client peer that connects to the rendezvous. Returns the
    /// index of the new peer in `self.peers`.
    pub async fn add_peer(&mut self, name: &str) -> Result<usize> {
        let dir = TempDir::new()?;
        write_config(dir.path(), &self.vault_id, Some(&self.rendezvous_url))?;

        let mut cmd = base_command(&self.binary, dir.path(), &self.vault_key_b64);
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn peer {}", name))?;

        wait_for_line(&mut child, |line| line.starts_with("watching "))
            .await
            .with_context(|| format!("peer {} did not reach watching state", name))?;

        spawn_log_drainer(&mut child, name);

        self.peers.push(Peer {
            name: name.to_string(),
            dir,
            proc: Some(child),
        });
        // Allow the freshly-connected peer to complete the initial sync round
        // before tests start writing.
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(self.peers.len() - 1)
    }

    pub fn peer(&self, idx: usize) -> &Peer {
        &self.peers[idx]
    }

    pub fn peer_by_name(&self, name: &str) -> Option<&Peer> {
        if name == self.rendezvous.name {
            return Some(&self.rendezvous);
        }
        self.peers.iter().find(|p| p.name == name)
    }

    /// Kill every spawned process. Drop also does this — call explicitly when
    /// you want a deterministic teardown point (or to surface errors).
    pub async fn shutdown(mut self) {
        if let Some(c) = self.rendezvous.proc.as_mut() {
            let _ = c.kill().await;
        }
        for peer in &mut self.peers {
            if let Some(c) = peer.proc.as_mut() {
                let _ = c.kill().await;
            }
        }
    }

    /// Port the rendezvous bound to. Stable across kill/restart so callers can
    /// exercise reconnect behavior without the OS picking a different port.
    pub fn rendezvous_port(&self) -> u16 {
        self.rendezvous_url
            .strip_prefix("ws://")
            .and_then(|rest| rest.rsplit_once(':'))
            .and_then(|(_, p)| p.parse().ok())
            .expect("malformed rendezvous URL")
    }

    /// Kill the rendezvous and wait for the OS to release its port. Storage
    /// dir is preserved so `restart_rendezvous()` can resume the same vault.
    pub async fn kill_rendezvous(&mut self) -> Result<()> {
        if let Some(c) = self.rendezvous.proc.as_mut() {
            let _ = c.kill().await;
            let _ = c.wait().await;
        }
        self.rendezvous.proc = None;
        Ok(())
    }

    /// Spawn a fresh rendezvous bound to the original port + storage dir.
    pub async fn restart_rendezvous(&mut self) -> Result<()> {
        let port = self.rendezvous_port();
        let mut cmd = base_command(&self.binary, self.rendezvous.dir.path(), &self.vault_key_b64);
        cmd.arg("--listen").arg(format!("127.0.0.1:{}", port));
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("respawn rendezvous")?;
        let bound_port = read_listen_port(&mut child)
            .await
            .context("waiting for rendezvous to rebind")?;
        if bound_port != port {
            bail!("rendezvous rebound on a different port: wanted {port}, got {bound_port}");
        }
        spawn_log_drainer(&mut child, "rendezvous");
        self.rendezvous.proc = Some(child);
        Ok(())
    }

    /// Like `new()` but does not spawn the rendezvous process. The vault
    /// state is created on disk and a free port is reserved (then released)
    /// so a peer can be pointed at it before the rendezvous is up. Use
    /// `start_rendezvous()` when ready.
    pub async fn prepared_offline() -> Result<Self> {
        let binary = locate_binary()?;
        let rendezvous_dir = TempDir::new()?;
        let storage = rendezvous_dir.path().join(".agentsync");
        let vault_key: VaultKey = generate_vault_key();
        let (vault, created) = Vault::create(CreateOptions {
            rendezvous_url: None,
            vault_key: Some(vault_key),
            storage_path: storage.clone(),
        })
        .await?;
        vault.flush().await?;
        drop(vault);

        let vault_id = created.vault_id;
        let vault_key_b64 = encode_key(&created.vault_key);
        write_config(rendezvous_dir.path(), &vault_id, None)?;

        // Reserve a free port by binding then immediately releasing it.
        let probe = std::net::TcpListener::bind("127.0.0.1:0")
            .context("reserve free port")?;
        let port = probe.local_addr()?.port();
        drop(probe);

        let rendezvous_url = format!("ws://127.0.0.1:{}", port);
        let rendezvous = Peer {
            name: "rendezvous".into(),
            dir: rendezvous_dir,
            proc: None,
        };

        Ok(E2EVault {
            binary,
            vault_id,
            vault_key_b64,
            rendezvous_url,
            rendezvous,
            peers: Vec::new(),
        })
    }

    /// Spawn the rendezvous process for an `E2EVault` created via
    /// `prepared_offline()`. Errors if the rendezvous is already running.
    pub async fn start_rendezvous(&mut self) -> Result<()> {
        if self.rendezvous.proc.is_some() {
            bail!("rendezvous already running");
        }
        let port = self.rendezvous_port();
        let mut cmd = base_command(&self.binary, self.rendezvous.dir.path(), &self.vault_key_b64);
        cmd.arg("--listen").arg(format!("127.0.0.1:{}", port));
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn rendezvous")?;
        let bound_port = read_listen_port(&mut child)
            .await
            .context("waiting for rendezvous to bind")?;
        if bound_port != port {
            bail!("rendezvous bound a different port: wanted {port}, got {bound_port}");
        }
        spawn_log_drainer(&mut child, "rendezvous");
        self.rendezvous.proc = Some(child);
        Ok(())
    }

    /// Spawn a peer without waiting for it to reach the watching state. The
    /// returned index can be used to address the peer; the caller is
    /// responsible for waiting on whatever it needs to observe.
    pub async fn add_peer_without_waiting(&mut self, name: &str) -> Result<usize> {
        let dir = TempDir::new()?;
        write_config(dir.path(), &self.vault_id, Some(&self.rendezvous_url))?;

        let mut cmd = base_command(&self.binary, dir.path(), &self.vault_key_b64);
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn peer {}", name))?;

        spawn_log_drainer(&mut child, name);

        self.peers.push(Peer {
            name: name.to_string(),
            dir,
            proc: Some(child),
        });
        Ok(self.peers.len() - 1)
    }
}

impl Drop for E2EVault {
    fn drop(&mut self) {
        // tokio::process::Child is configured with kill_on_drop, so dropping
        // the field is enough.
    }
}

impl Peer {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// Atomic-rename save: write to a `.tmp` sibling, then rename over the
    /// target. This is the pattern vim/VS Code/etc. use.
    pub fn save_atomic(&self, rel: &str, content: &str) -> Result<()> {
        let final_path = self.abs(rel);
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = final_path.with_file_name(format!(
            ".{}.atomic-tmp",
            final_path
                .file_name()
                .ok_or_else(|| anyhow!("no file name"))?
                .to_string_lossy()
        ));
        std::fs::write(&tmp, content.as_bytes())?;
        std::fs::rename(&tmp, &final_path)?;
        Ok(())
    }

    /// Truncate-then-write save: open the file with `O_TRUNC`, briefly leave
    /// it empty, then write the new content. Some editors and tools save this
    /// way, and it's the pattern that previously caused content loss.
    pub fn save_truncate(&self, rel: &str, content: &str) -> Result<()> {
        self.save_truncate_with_gap(rel, content, Duration::from_millis(40))
    }

    /// Truncate-then-write save with a configurable gap between truncate and
    /// write. Use this with a `gap` larger than the engine's debounce window
    /// to exercise the slow-editor case where the empty intermediate state
    /// has time to fire an inotify event of its own.
    pub fn save_truncate_with_gap(
        &self,
        rel: &str,
        content: &str,
        gap: Duration,
    ) -> Result<()> {
        let final_path = self.abs(rel);
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&final_path, b"")?;
        std::thread::sleep(gap);
        std::fs::write(&final_path, content.as_bytes())?;
        Ok(())
    }

    /// Append to a file (create if missing).
    pub fn save_append(&self, rel: &str, extra: &str) -> Result<()> {
        use std::io::Write;
        let final_path = self.abs(rel);
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&final_path)?;
        f.write_all(extra.as_bytes())?;
        Ok(())
    }

    pub fn delete(&self, rel: &str) -> Result<()> {
        let p = self.abs(rel);
        match std::fs::remove_file(&p) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn read(&self, rel: &str) -> Result<String> {
        Ok(std::fs::read_to_string(self.abs(rel))?)
    }

    pub fn try_read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.abs(rel)).ok()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.abs(rel).exists()
    }

    /// Poll until `rel` on this peer's disk equals `expected`, or `timeout`
    /// elapses. Returns whatever it last saw on timeout to aid debugging.
    pub async fn wait_for_content(
        &self,
        rel: &str,
        expected: &str,
        timeout: Duration,
    ) -> Result<()> {
        let start = Instant::now();
        let mut last = None;
        while start.elapsed() < timeout {
            match self.try_read(rel) {
                Some(s) if s == expected => return Ok(()),
                other => last = other,
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        bail!(
            "timeout waiting for {}/{} to become {:?}; last seen: {:?}",
            self.name,
            rel,
            short(expected),
            last.as_deref().map(short)
        )
    }

    pub async fn wait_for_missing(&self, rel: &str, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if !self.exists(rel) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        bail!(
            "timeout waiting for {}/{} to disappear",
            self.name,
            rel
        )
    }
}

fn short(s: &str) -> String {
    if s.len() <= 60 {
        s.to_string()
    } else {
        format!("{}…({} bytes)", &s[..60], s.len())
    }
}

fn write_config(dir: &Path, vault_id: &str, rendezvous_url: Option<&str>) -> Result<()> {
    let agentsync_dir = dir.join(".agentsync");
    std::fs::create_dir_all(&agentsync_dir)?;
    let mut cfg = format!(
        r#"[vault]
id = "{vault_id}"
"#
    );
    if let Some(url) = rendezvous_url {
        cfg.push_str(&format!("rendezvous_url = \"{}\"\n", url));
    }
    cfg.push_str(
        r#"
[key]
source = "env"
env_var = "AGENTSYNC_KEY"

[sync]
extensions = ["md", "markdown"]
exclude = [
    "**/.git/**",
    "**/node_modules/**",
    "**/.DS_Store",
    "**/.agentsync/**",
]
include = []
attachment_max_bytes = 10485760
text_file_max_bytes = 1048576
log_retention_days = 30
"#,
    );
    std::fs::write(agentsync_dir.join("config.toml"), cfg)?;
    Ok(())
}

fn base_command(binary: &Path, dir: &Path, key_b64: &str) -> Command {
    let mut cmd = Command::new(binary);
    cmd.current_dir(dir)
        .env("AGENTSYNC_KEY", key_b64)
        // Quiet by default; tests can set AGENTSYNC_LOG=debug for diagnostics.
        .env("AGENTSYNC_LOG", std::env::var("AGENTSYNC_LOG").unwrap_or_else(|_| "warn".into()))
        .kill_on_drop(true);
    cmd
}

/// Read the rendezvous's stdout until we see the "listening on ws://..." line,
/// then parse the bound port. Cancels after 5s.
async fn read_listen_port(child: &mut Child) -> Result<u16> {
    let stdout = child.stdout.take().context("rendezvous stdout missing")?;
    let mut reader = BufReader::new(stdout).lines();
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| anyhow!("timed out waiting for listen line"))?;
        let line = tokio::time::timeout(remaining, reader.next_line())
            .await
            .map_err(|_| anyhow!("timed out waiting for listen line"))??;
        let line = match line {
            Some(l) => l,
            None => bail!("rendezvous exited before printing listen line"),
        };
        if let Some(rest) = line.strip_prefix("listening on ws://") {
            // rest looks like "127.0.0.1:49152"
            let port_str = rest
                .rsplit_once(':')
                .map(|(_, p)| p)
                .ok_or_else(|| anyhow!("malformed listen line: {}", line))?;
            let port: u16 = port_str
                .parse()
                .with_context(|| format!("parse port from {}", port_str))?;
            // Put stdout back so the drainer can finish consuming it.
            let inner = reader.into_inner();
            child.stdout = Some(inner.into_inner());
            return Ok(port);
        }
    }
}

async fn wait_for_line(child: &mut Child, mut pred: impl FnMut(&str) -> bool) -> Result<()> {
    let stdout = child.stdout.take().context("peer stdout missing")?;
    let mut reader = BufReader::new(stdout).lines();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| anyhow!("timed out waiting for peer ready"))?;
        let line = tokio::time::timeout(remaining, reader.next_line())
            .await
            .map_err(|_| anyhow!("timed out"))??;
        let line = match line {
            Some(l) => l,
            None => bail!("peer exited unexpectedly"),
        };
        if pred(&line) {
            let inner = reader.into_inner();
            child.stdout = Some(inner.into_inner());
            return Ok(());
        }
    }
}

fn spawn_log_drainer(child: &mut Child, label: &str) {
    if let Some(out) = child.stdout.take() {
        let label = label.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if std::env::var("AGENTSYNC_E2E_VERBOSE").is_ok() {
                    eprintln!("[{}/stdout] {}", label, line);
                }
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let label = label.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if std::env::var("AGENTSYNC_E2E_VERBOSE").is_ok() {
                    eprintln!("[{}/stderr] {}", label, line);
                }
            }
        });
    }
}

// ---- binary location ----

static BINARY_BUILD: OnceLock<Result<PathBuf, String>> = OnceLock::new();

fn locate_binary() -> Result<PathBuf> {
    let result = BINARY_BUILD.get_or_init(|| build_binary().map_err(|e| e.to_string()));
    match result {
        Ok(p) => Ok(p.clone()),
        Err(e) => Err(anyhow!("could not locate agentsync binary: {}", e)),
    }
}

fn build_binary() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("AGENTSYNC_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        bail!("AGENTSYNC_BIN points at non-existent path: {}", p.display());
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .ancestors()
        .nth(2)
        .ok_or_else(|| anyhow!("cannot locate workspace root"))?
        .to_path_buf();

    // Try existing builds first to avoid rebuilding on every test invocation.
    for profile in ["debug", "release"] {
        let candidate = workspace
            .join("target")
            .join(profile)
            .join(if cfg!(windows) { "agentsync.exe" } else { "agentsync" });
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    // Build it. Use `cargo` from PATH; cargo invokes us with PATH set.
    let status = std::process::Command::new(
        std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
    )
    .args(["build", "--bin", "agentsync"])
    .current_dir(&workspace)
    .status()
    .context("invoke cargo build --bin agentsync")?;
    if !status.success() {
        bail!("cargo build --bin agentsync failed");
    }
    let candidate = workspace
        .join("target")
        .join("debug")
        .join(if cfg!(windows) { "agentsync.exe" } else { "agentsync" });
    if candidate.exists() {
        return Ok(candidate);
    }
    bail!("agentsync binary missing at {} after build", candidate.display());
}
