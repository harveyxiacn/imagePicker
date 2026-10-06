//! Implementation entry points; the work lives in the sibling modules.
use super::*;

pub fn create_renderer(prefer_gpu: bool) -> Box<dyn Renderer> {
    if prefer_gpu {
        if let Some(g) = gpu::GpuRenderer::probe() {
            return Box::new(g);
        }
    }
    Box::new(cpu::CpuRenderer)
}

pub fn gpu_adapter_name() -> Option<String> {
    gpu::GpuRenderer::probe().map(|g| g.adapter_name())
}

pub fn parse_cube(text: &str) -> Result<Lut3d> {
    cube::parse_cube(text)
}

pub fn auto_adjust(source: &RgbImage, ctx: &AutoContext, mode: AutoMode) -> Adjust {
    auto::auto_adjust(source, ctx, mode)
}

pub fn builtin_presets() -> Vec<(String, String, EditStack)> {
    presets::builtin_presets()
}

pub fn builtin_lut(id: &str) -> Option<Lut3d> {
    presets::builtin_lut(id)
}
