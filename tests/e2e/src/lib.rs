//! End-to-end harness that spawns the real `agentsync` CLI binary.
//!
//! Each `E2EVault` is a self-contained scenario:
//!   * `rendezvous` — a `--listen` peer in a fresh tempdir.
//!   * `peers`      — additional `agentsync` processes connected to it.
//!
//! Tests drive the peers via `Peer::save_atomic` / `Peer::save_truncate` /
//! `Peer::delete` and assert with `wait_for_content` / `wait_for_missing`.
//!
//! The harness builds the binary on first use (cached) and spawns it with
//! `kill_on_drop`, so dropped scenarios always clean up their child processes.

pub mod harness;

pub use harness::{E2EVault, Peer};
