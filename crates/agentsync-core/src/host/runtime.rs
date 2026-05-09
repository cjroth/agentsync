//! Async runtime primitives. Native targets back these with `tokio`; wasm
//! backs them with `wasm-bindgen-futures` + `futures::channel`. Vault and the
//! networking layer never reach for `tokio::*` directly — they go through
//! these traits so the same code runs on both targets.
//!
//! `?Send` is on the async-trait attributes because wasm32 has no real
//! threads and `JsValue`-bearing futures are not `Send`. Native impls satisfy
//! the bound trivially.

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use std::time::Duration;

/// Spawns detached background work. The returned [`SpawnHandle`] can be
/// dropped (task keeps running) or awaited (waits for completion).
pub trait Spawner: Send + Sync + 'static {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> SpawnHandle;
}

pub struct SpawnHandle {
    inner: Box<dyn SpawnHandleImpl>,
}

impl SpawnHandle {
    pub fn new(inner: Box<dyn SpawnHandleImpl>) -> Self {
        Self { inner }
    }

    pub fn abort(self) {
        self.inner.abort();
    }

    pub async fn join(self) {
        self.inner.join().await;
    }
}

#[async_trait(?Send)]
pub trait SpawnHandleImpl: Send + 'static {
    fn abort(self: Box<Self>);
    async fn join(self: Box<Self>);
}

/// Wall-clock + monotonic time. Native uses `std::time` and `tokio::time`;
/// wasm uses `js_sys::Date::now()` and a `Promise`-wrapped `setTimeout`.
pub trait Clock: Send + Sync + 'static {
    /// Milliseconds since the Unix epoch. Used for label timestamps and the
    /// TLS cert `not_before` / `not_after` fields.
    fn now_ms(&self) -> i64;
    /// Resolves after roughly `d` real time. May fire late on a busy
    /// runtime; callers must not rely on tight tolerance.
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>;
}
