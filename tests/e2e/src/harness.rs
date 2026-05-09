use agentsync_core::{
    render_authorized_keys, AuthorizedPeer, CreateOptions, Identity, Pubkey, Vault,
    AUTHORIZED_KEYS_FILE,
};
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
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
    pub rendezvous_url: String,
    pub rendezvous: Peer,
    pub peers: Vec<Peer>,
    /// Authorized peer pubkeys, kept in sync with the on-disk authorized_keys.
    authorized: Vec<AuthorizedPeer>,
}

/// One running `agentsync` process bound to a temp directory. `proc` is
/// `None` when the peer has been killed (or hasn't been spawned yet) but the
/// storage dir is being preserved for a later restart.
pub struct Peer {
    pub name: String,
    pub dir: TempDir,
    pub identity: Identity,
    proc: Option<Child>,
    /// Captures stderr lines as the child emits them. Tests can poll this
    /// via `Peer::stderr_dump` / `Peer::wait_for_stderr` to assert on log
    /// output without relying on `AGENTSYNC_E2E_VERBOSE`.
    stderr_lines: Arc<Mutex<Vec<String>>>,
}

impl E2EVault {
    /// Spin up a brand-new vault with a single listening peer named `rendezvous`.
    pub async fn new() -> Result<Self> {
        let binary = locate_binary()?;

        let (rendezvous_dir, rendezvous_identity, vault_id) =
            bootstrap_rendezvous_storage().await?;

        let mut authorized = vec![AuthorizedPeer {
            pubkey: rendezvous_identity.pubkey(),
            label: "rendezvous".into(),
        }];
        write_peers_md(rendezvous_dir.path(), &authorized)?;
        write_config(rendezvous_dir.path(), &vault_id, None)?;

        let mut cmd = base_command(&binary, rendezvous_dir.path());
        cmd.arg("--listen").arg("127.0.0.1:0");
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn rendezvous")?;

        let port = read_listen_port(&mut child)
            .await
            .context("waiting for rendezvous to bind")?;
        let rendezvous_url = format!("wss://127.0.0.1:{}", port);

        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        spawn_log_drainer(&mut child, "rendezvous", Some(stderr_lines.clone()));

        let rendezvous = Peer {
            name: "rendezvous".into(),
            dir: rendezvous_dir,
            identity: rendezvous_identity.clone(),
            proc: Some(child),
            stderr_lines,
        };

        // Sort the authorized list deterministically (cheap, helps tests that
        // diff authorized_keys content).
        authorized.sort_by(|a, b| a.label.cmp(&b.label));

        Ok(E2EVault {
            binary,
            vault_id,
            rendezvous_url,
            rendezvous,
            peers: Vec::new(),
            authorized,
        })
    }

    /// Add a new client peer that connects to the rendezvous. Generates a
    /// fresh ed25519 identity, authorizes it on the hub (by appending to
    /// authorized_keys on the hub's disk), and waits for the spawned peer to reach
    /// the watching state.
    pub async fn add_peer(&mut self, name: &str) -> Result<usize> {
        let dir = TempDir::new()?;
        let identity = Identity::generate();

        // Persist the peer's identity at the default per-vault location.
        let id_path = dir.path().join(".agentsync").join("identity");
        identity
            .save_to_file(&id_path)
            .context("write peer identity")?;

        // Authorize the peer on the hub side BEFORE the connect attempt; the
        // hub's file watcher ingests authorized_keys within the debounce window.
        self.authorize_peer(name, &identity.pubkey()).await?;

        write_config(dir.path(), &self.vault_id, Some(&self.rendezvous_url))?;

        let mut cmd = base_command(&self.binary, dir.path());
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn peer {}", name))?;

        wait_for_line(&mut child, |line| line.starts_with("watching "))
            .await
            .with_context(|| format!("peer {} did not reach watching state", name))?;

        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        spawn_log_drainer(&mut child, name, Some(stderr_lines.clone()));

        self.peers.push(Peer {
            name: name.to_string(),
            dir,
            identity,
            proc: Some(child),
            stderr_lines,
        });
        // Allow the freshly-connected peer to complete the initial sync round
        // before tests start writing.
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(self.peers.len() - 1)
    }

    /// Append `pubkey` to the hub's authorized_keys (on disk and in memory) so the
    /// hub will accept connections from a peer holding the matching identity.
    pub async fn authorize_peer(&mut self, label: &str, pubkey: &Pubkey) -> Result<()> {
        if self.authorized.iter().any(|p| p.pubkey == *pubkey) {
            return Ok(());
        }
        self.authorized.push(AuthorizedPeer {
            pubkey: *pubkey,
            label: label.to_string(),
        });
        write_peers_md(self.rendezvous.dir.path(), &self.authorized)?;
        // Give the rendezvous's fs watcher a moment to ingest the change. The
        // engine's debounce window is ~150ms; round up generously.
        tokio::time::sleep(Duration::from_millis(400)).await;
        Ok(())
    }

    /// Remove a peer's pubkey from the hub's authorized_keys. After the rendezvous
    /// re-evaluates authorizations, any currently-connected peer with that
    /// pubkey is dropped.
    pub async fn deauthorize_peer(&mut self, pubkey: &Pubkey) -> Result<()> {
        self.authorized.retain(|p| p.pubkey != *pubkey);
        write_peers_md(self.rendezvous.dir.path(), &self.authorized)?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        Ok(())
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

    pub fn rendezvous_port(&self) -> u16 {
        self.rendezvous_url
            .strip_prefix("wss://")
            .or_else(|| self.rendezvous_url.strip_prefix("ws://"))
            .and_then(|rest| rest.rsplit_once(':'))
            .and_then(|(_, p)| p.parse().ok())
            .expect("malformed rendezvous URL")
    }

    pub async fn kill_rendezvous(&mut self) -> Result<()> {
        if let Some(c) = self.rendezvous.proc.as_mut() {
            let _ = c.kill().await;
            let _ = c.wait().await;
        }
        self.rendezvous.proc = None;
        Ok(())
    }

    pub async fn restart_rendezvous(&mut self) -> Result<()> {
        let port = self.rendezvous_port();
        let mut cmd = base_command(&self.binary, self.rendezvous.dir.path());
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
        spawn_log_drainer(&mut child, "rendezvous", Some(self.rendezvous.stderr_lines.clone()));
        self.rendezvous.proc = Some(child);
        Ok(())
    }

    pub async fn prepared_offline() -> Result<Self> {
        let binary = locate_binary()?;

        let (rendezvous_dir, rendezvous_identity, vault_id) =
            bootstrap_rendezvous_storage().await?;

        let authorized = vec![AuthorizedPeer {
            pubkey: rendezvous_identity.pubkey(),
            label: "rendezvous".into(),
        }];
        write_peers_md(rendezvous_dir.path(), &authorized)?;
        write_config(rendezvous_dir.path(), &vault_id, None)?;

        // Reserve a free port by binding then immediately releasing it.
        let probe = std::net::TcpListener::bind("127.0.0.1:0")
            .context("reserve free port")?;
        let port = probe.local_addr()?.port();
        drop(probe);

        let rendezvous_url = format!("wss://127.0.0.1:{}", port);
        let rendezvous = Peer {
            name: "rendezvous".into(),
            dir: rendezvous_dir,
            identity: rendezvous_identity,
            proc: None,
            stderr_lines: Arc::new(Mutex::new(Vec::new())),
        };

        Ok(E2EVault {
            binary,
            vault_id,
            rendezvous_url,
            rendezvous,
            peers: Vec::new(),
            authorized,
        })
    }

    pub async fn start_rendezvous(&mut self) -> Result<()> {
        if self.rendezvous.proc.is_some() {
            bail!("rendezvous already running");
        }
        let port = self.rendezvous_port();
        let mut cmd = base_command(&self.binary, self.rendezvous.dir.path());
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
        spawn_log_drainer(&mut child, "rendezvous", Some(self.rendezvous.stderr_lines.clone()));
        self.rendezvous.proc = Some(child);
        Ok(())
    }

    /// Spawn an authorized peer process without waiting for it to reach
    /// `watching` — handy for tests that want to observe early-startup
    /// behavior (e.g. handshake errors). The peer's pubkey is added to
    /// authorized_keys before spawning.
    pub async fn add_peer_without_waiting(&mut self, name: &str) -> Result<usize> {
        let dir = TempDir::new()?;
        let identity = Identity::generate();
        let id_path = dir.path().join(".agentsync").join("identity");
        identity
            .save_to_file(&id_path)
            .context("write peer identity")?;

        self.authorize_peer(name, &identity.pubkey()).await?;
        write_config(dir.path(), &self.vault_id, Some(&self.rendezvous_url))?;

        let mut cmd = base_command(&self.binary, dir.path());
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn peer {}", name))?;

        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        spawn_log_drainer(&mut child, name, Some(stderr_lines.clone()));

        self.peers.push(Peer {
            name: name.to_string(),
            dir,
            identity,
            proc: Some(child),
            stderr_lines,
        });
        Ok(self.peers.len() - 1)
    }

    /// Spawn a peer whose pubkey is *not* added to authorized_keys. The returned
    /// process is expected to fail at handshake time; tests should observe
    /// its stderr or exit code.
    pub async fn add_unauthorized_peer(&mut self, name: &str) -> Result<usize> {
        let dir = TempDir::new()?;
        let identity = Identity::generate();
        let id_path = dir.path().join(".agentsync").join("identity");
        identity
            .save_to_file(&id_path)
            .context("write peer identity")?;
        write_config(dir.path(), &self.vault_id, Some(&self.rendezvous_url))?;

        let mut cmd = base_command(&self.binary, dir.path());
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn unauthorized peer {}", name))?;

        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        spawn_log_drainer(&mut child, name, Some(stderr_lines.clone()));

        self.peers.push(Peer {
            name: name.to_string(),
            dir,
            identity,
            proc: Some(child),
            stderr_lines,
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

    /// Snapshot of every stderr line the child has emitted so far, joined
    /// with newlines. Useful for printing in failure messages.
    pub fn stderr_dump(&self) -> String {
        self.stderr_lines.lock().unwrap().join("\n")
    }

    /// Wait until any stderr line satisfies `pred` or the timeout elapses.
    /// Returns the matching line on success.
    pub async fn wait_for_stderr(
        &self,
        mut pred: impl FnMut(&str) -> bool,
        timeout: Duration,
    ) -> Option<String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            {
                let lines = self.stderr_lines.lock().unwrap();
                if let Some(found) = lines.iter().rev().find(|l| pred(l)) {
                    return Some(found.clone());
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        None
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    pub fn pubkey(&self) -> Pubkey {
        self.identity.pubkey()
    }

    /// True when the underlying process has exited (or has been reaped).
    pub async fn is_alive(&mut self) -> bool {
        match self.proc.as_mut() {
            Some(c) => c.try_wait().map(|s| s.is_none()).unwrap_or(false),
            None => false,
        }
    }

    pub async fn wait_for_exit(&mut self, timeout: Duration) -> Result<std::process::ExitStatus> {
        let proc = self
            .proc
            .as_mut()
            .ok_or_else(|| anyhow!("peer {} has no live process", self.name))?;
        let status = tokio::time::timeout(timeout, proc.wait())
            .await
            .with_context(|| format!("peer {} did not exit within {:?}", self.name, timeout))??;
        Ok(status)
    }

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

    pub fn save_truncate(&self, rel: &str, content: &str) -> Result<()> {
        self.save_truncate_with_gap(rel, content, Duration::from_millis(40))
    }

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

async fn bootstrap_rendezvous_storage() -> Result<(TempDir, Identity, String)> {
    let dir = TempDir::new()?;
    let storage = dir.path().join(".agentsync");
    let identity = Identity::generate();
    // Stash the identity at the per-vault default location.
    let id_path = storage.join("identity");
    identity
        .save_to_file(&id_path)
        .context("write rendezvous identity")?;
    let (vault, created) = Vault::create(CreateOptions {
        rendezvous_url: None,
        identity: Some(identity.clone()),
        storage_path: storage.clone(),
    })
    .await?;
    vault.flush().await?;
    drop(vault);
    Ok((dir, identity, created.vault_id))
}

fn write_peers_md(dir: &Path, peers: &[AuthorizedPeer]) -> Result<()> {
    let body = render_authorized_keys(peers);
    std::fs::write(dir.join(AUTHORIZED_KEYS_FILE), body)?;
    Ok(())
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
[identity]
path = ".agentsync/identity"

[sync]
extensions = ["md", "markdown"]
include = []
attachment_max_bytes = 10485760
text_file_max_bytes = 1048576
log_retention_days = 30
"#,
    );
    std::fs::write(agentsync_dir.join("config.toml"), cfg)?;
    Ok(())
}

fn base_command(binary: &Path, dir: &Path) -> Command {
    let mut cmd = Command::new(binary);
    cmd.current_dir(dir)
        // Default to `info` so peer-add/remove notices land in stderr;
        // tests that need verbose output can override via AGENTSYNC_LOG.
        .env(
            "AGENTSYNC_LOG",
            std::env::var("AGENTSYNC_LOG").unwrap_or_else(|_| "info".into()),
        )
        .kill_on_drop(true);
    cmd
}

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
        if let Some(rest) = line
            .strip_prefix("listening on ws://")
            .or_else(|| line.strip_prefix("listening on wss://"))
        {
            let port_str = rest
                .rsplit_once(':')
                .map(|(_, p)| p)
                .ok_or_else(|| anyhow!("malformed listen line: {}", line))?;
            let port: u16 = port_str
                .parse()
                .with_context(|| format!("parse port from {}", port_str))?;
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

fn spawn_log_drainer(
    child: &mut Child,
    label: &str,
    stderr_sink: Option<Arc<Mutex<Vec<String>>>>,
) {
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
                if let Some(sink) = stderr_sink.as_ref() {
                    sink.lock().unwrap().push(line.clone());
                }
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

    for profile in ["debug", "release"] {
        let candidate = workspace
            .join("target")
            .join(profile)
            .join(if cfg!(windows) { "agentsync.exe" } else { "agentsync" });
        if candidate.exists() {
            return Ok(candidate);
        }
    }

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
