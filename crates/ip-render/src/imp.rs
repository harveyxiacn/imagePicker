//! Implementation stubs — to be filled in by the render-engine work package.
use super::*;

struct Stub;

impl Renderer for Stub {
    fn backend(&self) -> Backend {
        Backend::Cpu
    }
    fn render(&self, _req: &RenderRequest<'_>) -> Result<RgbImage> {
        anyhow::bail!("render: not implemented")
    }
}

pub fn create_renderer(_prefer_gpu: bool) -> Box<dyn Renderer> {
    Box::new(Stub)
}

pub fn parse_cube(_text: &str) -> Result<Lut3d> {
    anyhow::bail!("parse_cube: not implemented")
}

pub fn auto_adjust(_source: &RgbImage, _ctx: &AutoContext, _mode: AutoMode) -> Adjust {
    Adjust::default()
}

pub fn builtin_presets() -> Vec<(String, String, EditStack)> {
    Vec::new()
}
