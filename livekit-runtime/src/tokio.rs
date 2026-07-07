// Copyright 2025 LiveKit, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::future::Future;
use std::pin::Pin;

pub use tokio::net::TcpStream;
pub use tokio::time::interval;
pub use tokio::time::sleep;
pub use tokio::time::timeout;
pub use tokio::time::Instant;
pub use tokio::time::MissedTickBehavior;
pub use tokio_stream::Stream;

pub type JoinHandle<T> = TokioJoinHandle<T>;
pub type Interval = tokio::time::Interval;

/// Stored runtime handle for spawn(). Set this once during initialization.
static RUNTIME: std::sync::OnceLock<tokio::runtime::Handle> = std::sync::OnceLock::new();

/// Store a runtime handle that spawn() will fall back to when no runtime
/// context is active on the calling thread.
pub fn set_runtime(handle: tokio::runtime::Handle) {
    let _ = RUNTIME.set(handle);
}

#[derive(Debug)]
pub struct TokioJoinHandle<T> {
    handle: tokio::task::JoinHandle<T>,
}

pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    // First, try to get the current runtime handle. If that fails,
    // fall back to the stored runtime. Panic if neither is available.
    let handle = {
        let h = tokio::runtime::Handle::try_current()
            .ok()
            .or_else(|| RUNTIME.get().cloned());
        match h {
            Some(handle) => handle.spawn(future),
            None => panic!(
                "livekit_runtime::spawn: no tokio runtime available. \
                 Call set_runtime() first or enter a runtime context."
            ),
        }
    };
    TokioJoinHandle { handle }
}

impl<T> Future for TokioJoinHandle<T> {
    type Output = T;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = &mut *self;
        match Pin::new(&mut this.handle).poll(cx) {
            std::task::Poll::Ready(Ok(v)) => std::task::Poll::Ready(v),
            std::task::Poll::Ready(Err(e)) => {
                // Task panicked — resume the panic on the awaiting thread
                std::panic::resume_unwind(e.into_panic())
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}
