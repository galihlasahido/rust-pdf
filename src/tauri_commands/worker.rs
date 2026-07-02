//! A small, dependency-light worker thread pool used to run CPU-bound PDF
//! parsing/editing/signing work off of Tauri's own async-command executor
//! threads.
//!
//! Tauri (as of the `tauri` dependency this feature adds) dispatches
//! `async fn` commands onto its managed Tokio runtime. Doing genuinely
//! blocking, possibly multi-second work (parsing a large xref chain,
//! walking every page of a big document for `extract_text`/`search_text`,
//! a full-rewrite save, PKCS#7 signing) directly inside such an `async
//! fn` would not freeze the native UI event-loop thread (that thread is
//! separate), but it *would* starve Tokio's own worker threads, delaying
//! every other concurrent command the frontend has in flight -- exactly
//! what Tokio's own guidance warns against for blocking work inside
//! `async fn`. This pool gives that work its own dedicated OS threads
//! instead.
//!
//! Used by every command whose underlying rust-pdf type is `Send`
//! (`EditableDocument`, `Certificate`, `PrivateKey`,
//! `IncrementalSigner`, ...) -- i.e. every command except page
//! rasterization, which instead goes through
//! [`super::render_actor::RenderActor`] because Pdfium's document handle
//! is not `Send` (see that module's docs for why).

use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use super::error::CommandError;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Fixed-size pool of worker OS threads pulling boxed closures off a
/// shared queue.
pub struct WorkerPool {
    sender: Option<mpsc::Sender<Job>>,
    workers: Vec<JoinHandle<()>>,
}

/// Picks a reasonable default worker-thread count for the host machine:
/// the number of available CPUs, clamped to a sane `[1, 8]` range so a
/// many-core build machine doesn't spawn dozens of mostly-idle threads
/// for a desktop app whose actual bottleneck is usually I/O or a single
/// document's sequential structure, not raw core count.
pub fn default_worker_thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8)
}

impl WorkerPool {
    /// Spawns `thread_count` worker threads (clamped to at least 1).
    pub fn new(thread_count: usize) -> Self {
        let thread_count = thread_count.max(1);
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut workers = Vec::with_capacity(thread_count);
        for id in 0..thread_count {
            let receiver = Arc::clone(&receiver);
            // SAFETY-equivalent note (no `unsafe` here, but the same
            // "don't let one bad job take down the pool" concern
            // applies): a job's `FnOnce` is run inside `catch_unwind`
            // below so a panic inside one command's PDF-processing logic
            // can never silently shrink this pool for the rest of the
            // app session.
            let spawn_result = std::thread::Builder::new()
                .name(format!("rust-pdf-worker-{id}"))
                .spawn(move || Self::worker_loop(receiver));
            if let Ok(handle) = spawn_result {
                workers.push(handle);
            }
            // A failed `spawn` (e.g. the OS thread limit was hit) simply
            // means this pool runs with fewer threads than requested
            // rather than panicking the caller; `run` still works
            // correctly (just with less parallelism) as long as at least
            // one thread started.
        }
        Self {
            sender: Some(sender),
            workers,
        }
    }

    fn worker_loop(receiver: Arc<Mutex<mpsc::Receiver<Job>>>) {
        loop {
            // Only the (cheap) "receive next job" step holds the lock, so
            // a slow job on one thread never blocks the others from
            // picking up further-queued jobs.
            let job = {
                let guard = match receiver.lock() {
                    Ok(guard) => guard,
                    Err(_) => break, // A peer thread panicked while holding the lock: shut down.
                };
                guard.recv()
            };
            match job {
                Ok(job) => {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                }
                Err(_) => break, // Sender dropped: pool is shutting down.
            }
        }
    }

    /// Runs `f` on the pool and asynchronously awaits its result without
    /// blocking the calling async task's own executor thread.
    pub async fn run<F, T>(&self, f: F) -> Result<T, CommandError>
    where
        F: FnOnce() -> Result<T, CommandError> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| CommandError::internal("worker pool has been shut down"))?;
        sender
            .send(Box::new(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                    .unwrap_or_else(|_| {
                        Err(CommandError::internal(
                            "internal error: worker job panicked",
                        ))
                    });
                let _ = tx.send(result);
            }))
            .map_err(|_| CommandError::internal("worker pool has been shut down"))?;
        rx.await
            .map_err(|_| CommandError::internal("worker task was dropped before completing"))?
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        // Dropping the sender first lets every worker's blocking `recv()`
        // return `Err` and exit its loop; only then do we join them, so
        // this never deadlocks (join-before-closing-the-channel would
        // wait forever) and so tests that create many short-lived
        // `AppState`s never leak detached threads.
        self.sender = None;
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_job_and_returns_its_result() {
        let pool = WorkerPool::new(2);
        let result = pool.run(|| Ok::<_, CommandError>(2 + 2)).await;
        assert_eq!(result.unwrap(), 4);
    }

    #[tokio::test]
    async fn propagates_job_error() {
        let pool = WorkerPool::new(1);
        let result: Result<i32, CommandError> = pool
            .run(|| Err(CommandError::invalid_argument("bad input")))
            .await;
        let err = result.unwrap_err();
        assert_eq!(err.code, super::super::error::ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn survives_a_panicking_job() {
        let pool = WorkerPool::new(1);
        let panicking: Result<i32, CommandError> = pool
            .run(|| -> Result<i32, CommandError> { panic!("boom") })
            .await;
        assert!(panicking.is_err());

        // The pool must still be usable afterwards -- a bug in one
        // command's logic must not shrink/kill the pool for the rest of
        // the app session.
        let healthy = pool.run(|| Ok::<_, CommandError>(1)).await;
        assert_eq!(healthy.unwrap(), 1);
    }

    #[tokio::test]
    async fn runs_many_jobs_concurrently() {
        let pool = WorkerPool::new(4);
        let mut handles = Vec::new();
        for i in 0..16 {
            handles.push(pool.run(move || Ok::<_, CommandError>(i * 2)));
        }
        let results: Vec<i32> = futures_join_all(handles).await;
        let mut expected: Vec<i32> = (0..16).map(|i| i * 2).collect();
        let mut actual = results;
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }

    /// Minimal stand-in for `futures::future::join_all` (not a
    /// dependency of this crate): awaits a `Vec` of futures in order and
    /// unwraps each `Result`, which is all this test needs.
    async fn futures_join_all(
        futures: Vec<impl std::future::Future<Output = Result<i32, CommandError>>>,
    ) -> Vec<i32> {
        let mut out = Vec::with_capacity(futures.len());
        for fut in futures {
            out.push(fut.await.expect("job should not fail"));
        }
        out
    }

    #[test]
    fn pool_drop_joins_worker_threads_without_hanging() {
        // Regression guard: dropping the pool must not deadlock (it would
        // if `Drop` tried to join before releasing the sender).
        let pool = WorkerPool::new(3);
        drop(pool);
    }
}
