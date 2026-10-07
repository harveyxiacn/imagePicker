//! A renderer whose (slow) GPU initialisation happens on a background thread, so the server
//! answers `/api/health` as soon as the catalog is open. Callers that need the renderer before
//! it is ready simply wait for the one shared initialisation.

use std::sync::{Arc, Condvar, Mutex, OnceLock};

use ip_render::{Backend, RenderRequest, Renderer};

/// Number of warm-up threads currently inside renderer creation (GPU driver code).
static WARMING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// Decrements `WARMING` when initialisation ends, even by panic.
struct WarmGuard;

impl Drop for WarmGuard {
    fn drop(&mut self) {
        let mut n = WARMING.0.lock().unwrap_or_else(|e| e.into_inner());
        *n -= 1;
        WARMING.1.notify_all();
    }
}

/// Blocks until no background renderer initialisation is running. Exiting the process (or
/// tearing down `Core`) while a thread is still inside the Vulkan/GL driver corrupts the
/// heap in the driver's atexit handlers, so shutdown paths must call this first.
pub fn wait_for_warmup() {
    let mut n = WARMING.0.lock().unwrap_or_else(|e| e.into_inner());
    while *n > 0 {
        n = WARMING.1.wait(n).unwrap_or_else(|e| e.into_inner());
    }
}

pub struct LazyRenderer {
    prefer_gpu: bool,
    cell: OnceLock<Box<dyn Renderer>>,
}

impl LazyRenderer {
    /// Starts creating the renderer in the background and returns immediately.
    pub fn spawn(prefer_gpu: bool) -> Arc<LazyRenderer> {
        let r = Arc::new(LazyRenderer {
            prefer_gpu,
            cell: OnceLock::new(),
        });
        let bg = r.clone();
        *WARMING.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        let spawned = std::thread::Builder::new()
            .name("ip-render-warmup".into())
            .spawn(move || {
                let _guard = WarmGuard;
                bg.get();
            });
        if spawned.is_err() {
            drop(WarmGuard);
            tracing::warn!("could not start the renderer warm-up thread; creating it on first use");
        }
        r
    }

    fn get(&self) -> &dyn Renderer {
        self.cell
            .get_or_init(|| ip_render::create_renderer(self.prefer_gpu))
            .as_ref()
    }
}

impl Renderer for LazyRenderer {
    fn backend(&self) -> Backend {
        self.get().backend()
    }

    fn render(&self, req: &RenderRequest<'_>) -> ip_render::Result<ip_render::RgbImage> {
        self.get().render(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_for_warmup_blocks_until_init_is_done() {
        let r = LazyRenderer::spawn(false);
        wait_for_warmup();
        assert!(
            r.cell.get().is_some(),
            "warm-up finished before wait returned"
        );
    }
}
