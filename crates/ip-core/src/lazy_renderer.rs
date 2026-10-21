//! A renderer whose (slow) GPU initialisation happens on a background thread.
//!
//! Opening the catalog must not wait for a graphics driver. `wgpu::Instance::new` loads the
//! platform driver — on Windows the Vulkan ICD, a ~180 MB DLL, under the loader lock — and that
//! takes tens of seconds on a loaded machine. Initialisation therefore starts on the *first
//! render*, not when the `Core` opens: at open time it raced the first requests, and anything
//! that needed the same loader lock (`roots::default_roots`, which the auth middleware calls,
//! touches shell32) sat behind it for the whole driver load.
//!
//! A caller that renders before initialisation finishes is served by the CPU renderer for that
//! call instead of waiting. Blocking there is what made the suite stall: a render happens on a
//! tokio worker, and a blocked worker stops that runtime's timers, so no `tokio::time::timeout`
//! around it can ever fire.
//!
//! Shutdown still waits for the warm-up thread — exiting the process while a thread is inside the
//! Vulkan/GL driver corrupts the heap in the driver's atexit handlers — but only for *this*
//! instance's thread, and only for a bounded time.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ip_render::{Backend, RenderRequest, Renderer, RgbImage};

/// How long shutdown lets a driver initialisation finish before giving up. Initialisation
/// normally takes well under a second; the wait exists to avoid tearing down mid-driver, not to
/// pin the process to a driver that never returns.
const WARMUP_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Warm {
    /// Nothing has asked for a render yet.
    Idle,
    /// A warm-up thread is creating the renderer.
    Running,
    /// The renderer is in `cell` (or creation failed and it never will be).
    Done,
}

struct Inner {
    prefer_gpu: bool,
    /// The real renderer, created by the warm-up thread.
    cell: OnceLock<Box<dyn Renderer>>,
    /// Served while `cell` is still empty. The CPU path is the documented fallback for "no
    /// usable adapter", so a render during warm-up returns what a GPU-less machine would.
    cpu: Box<dyn Renderer>,
    state: Mutex<Warm>,
    done: Condvar,
}

pub struct LazyRenderer {
    inner: Arc<Inner>,
}

impl LazyRenderer {
    pub fn new(prefer_gpu: bool) -> Arc<LazyRenderer> {
        Arc::new(LazyRenderer {
            inner: Arc::new(Inner {
                prefer_gpu,
                cell: OnceLock::new(),
                cpu: ip_render::create_renderer(false),
                state: Mutex::new(Warm::Idle),
                done: Condvar::new(),
            }),
        })
    }

    /// Starts the background initialisation. Idempotent and non-blocking: the thread that wins
    /// creates the renderer, every other caller carries on with the CPU one.
    fn start(&self) {
        {
            let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if *s != Warm::Idle {
                return;
            }
            *s = Warm::Running;
        }
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new()
            .name("ip-render-warmup".into())
            .spawn(move || {
                // `create_renderer` already maps driver failures to the CPU renderer; the guard
                // only covers a panic leaving the state at `Running` forever.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    inner
                        .cell
                        .get_or_init(|| ip_render::create_renderer(inner.prefer_gpu));
                }));
                *inner.state.lock().unwrap_or_else(|e| e.into_inner()) = Warm::Done;
                inner.done.notify_all();
            });
        if let Err(e) = spawned {
            // No thread available: do it here rather than leave the cell empty forever. This is
            // the old blocking behaviour, on the one caller that asked for a render.
            tracing::warn!(error = %e, "could not start the renderer warm-up thread; creating it now");
            let inner = &self.inner;
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                inner
                    .cell
                    .get_or_init(|| ip_render::create_renderer(inner.prefer_gpu));
            }));
            *self.inner.state.lock().unwrap_or_else(|e| e.into_inner()) = Warm::Done;
        }
    }

    /// Waits (bounded) for this instance's warm-up thread. Called from `Core::drop`.
    pub fn wait_for_warmup(&self) {
        let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if *s != Warm::Running {
            return;
        }
        let deadline = Instant::now() + WARMUP_WAIT;
        while *s == Warm::Running {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                tracing::warn!(
                    secs = WARMUP_WAIT.as_secs(),
                    "renderer warm-up still running at shutdown; exiting anyway"
                );
                return;
            }
            s = self
                .inner
                .done
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Renderer for LazyRenderer {
    /// The backend in use, or — before anything has been initialised — the one this renderer is
    /// configured to prefer. The distinction matters: `Settings::apply_render_backend` compares
    /// this with the value it is asked to switch to, and a start-up "gpu" (or a switch to it) must
    /// read as "already there" rather than create a second renderer synchronously, which would
    /// put the driver load it exists to avoid back on the request thread.
    fn backend(&self) -> Backend {
        match self.inner.cell.get() {
            Some(r) => r.backend(),
            None if self.inner.prefer_gpu => Backend::Gpu,
            None => Backend::Cpu,
        }
    }

    fn render(&self, req: &RenderRequest<'_>) -> ip_render::Result<RgbImage> {
        match self.inner.cell.get() {
            Some(r) => r.render(req),
            None => {
                self.start();
                self.inner.cpu.render(req)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_for_warmup_returns_once_init_is_done() {
        let r = LazyRenderer::new(false);
        r.start(); // what a render does
        r.wait_for_warmup();
        assert!(
            r.inner.cell.get().is_some(),
            "warm-up finished before wait returned"
        );
        assert_eq!(r.backend(), Backend::Cpu);
    }

    /// Before initialisation the reported backend is the configured preference, so a start-up
    /// "gpu" setting does not make `apply_render_backend` build a second, blocking renderer.
    #[test]
    fn backend_before_init_reports_the_preference() {
        assert_eq!(LazyRenderer::new(true).backend(), Backend::Gpu);
        assert_eq!(LazyRenderer::new(false).backend(), Backend::Cpu);
        assert!(LazyRenderer::new(true).inner.cell.get().is_none());
    }

    #[test]
    fn wait_for_warmup_is_a_no_op_before_any_render() {
        let r = LazyRenderer::new(true);
        // Never rendered: shutdown must not sit on a driver that was never loaded.
        r.wait_for_warmup();
        assert!(r.inner.cell.get().is_none());
    }
}
