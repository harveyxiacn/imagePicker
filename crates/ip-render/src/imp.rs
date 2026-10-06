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
    tidy(auto::auto_adjust(source, ctx, mode))
}

/// Round suggestions to slider granularity so they display (and serialise) cleanly:
/// exposure to 0.01 EV, temp to 10 K, everything else to whole units.
fn tidy(mut a: Adjust) -> Adjust {
    let whole = |v: &mut f32| *v = v.round() + 0.0; // + 0.0 turns -0.0 into 0.0
    a.exposure = (a.exposure * 100.0).round() / 100.0 + 0.0;
    a.temp = (a.temp / 10.0).round() * 10.0 + 0.0;
    for v in [
        &mut a.contrast,
        &mut a.highlights,
        &mut a.shadows,
        &mut a.whites,
        &mut a.blacks,
        &mut a.tint,
        &mut a.vibrance,
        &mut a.saturation,
        &mut a.clarity,
        &mut a.dehaze,
    ] {
        whole(v);
    }
    a
}

pub fn builtin_presets() -> Vec<(String, String, EditStack)> {
    presets::builtin_presets()
}

pub fn builtin_lut(id: &str) -> Option<Lut3d> {
    presets::builtin_lut(id)
}
