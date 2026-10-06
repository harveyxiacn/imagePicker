//! A renderer whose (slow) GPU initialisation happens on a background thread, so the server
//! answers `/api/health` as soon as the catalog is open. Callers that need the renderer before
//! it is ready simply wait for the one shared initialisation.

use std::sync::{Arc, OnceLock};

use ip_render::{Backend, RenderRequest, Renderer};

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
        let spawned = std::thread::Builder::new()
            .name("ip-render-warmup".into())
            .spawn(move || {
                bg.get();
            });
        if spawned.is_err() {
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
