//! Native runtime backed by tokio.

use crate::host::runtime::{Clock, SpawnHandle, SpawnHandleImpl, Spawner};
use async_trait::async_trait;
use futures_util::future::BoxFuture;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

pub struct TokioSpawner;

impl Spawner for TokioSpawner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> SpawnHandle {
        let handle = tokio::spawn(fut);
        SpawnHandle::new(Box::new(TokioSpawnHandle {
            handle: Some(handle),
        }))
    }
}

struct TokioSpawnHandle {
    handle: Option<JoinHandle<()>>,
}

#[async_trait(?Send)]
impl SpawnHandleImpl for TokioSpawnHandle {
    fn abort(mut self: Box<Self>) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
    async fn join(mut self: Box<Self>) {
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            tokio::time::sleep(d).await;
        })
    }
}
