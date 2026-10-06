//! wgpu compute renderer (DX12 / Vulkan / Metal). Same maths as the CPU path: the
//! shaders in `shaders/*.wgsl` are ports of `cpu.rs`, and both consume the same `Plan`.

use std::sync::{Arc, Mutex, OnceLock};

use bytemuck::{Pod, Zeroable};

use crate::color::dec_lut;
use crate::geom::Geo;
use crate::prep::{self, Plan, REC};
use crate::*;

const RENDER_WGSL: &str = include_str!("../shaders/render.wgsl");
const LAYERS_WGSL: &str = include_str!("../shaders/layers.wgsl");
/// Pixels per output tile (64 MB of packed RGBA).
const MAX_TILE_PX: u64 = 16 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct Params {
    row0: [u32; 4],
    row1: [u32; 4],
    geo0: [f32; 4],
    geo1: [f32; 4],
    row2: [u32; 4],
    row3: [u32; 4],
    misc: [f32; 4],
    luts: [[f32; 4]; 4],
    sig: [f32; 4],
    wrp: [u32; 4],
    rect: [f32; 4],
}

struct Device {
    device: wgpu::Device,
    queue: wgpu::Queue,
    bgl_main: wgpu::BindGroupLayout,
    bgl_layer: wgpu::BindGroupLayout,
    p_final: wgpu::ComputePipeline,
    p_proxy: wgpu::ComputePipeline,
    p_layer_a: wgpu::ComputePipeline,
    p_layer_b: wgpu::ComputePipeline,
    p_beauty_a: wgpu::ComputePipeline,
    p_beauty_b: wgpu::ComputePipeline,
    p_beauty_c: wgpu::ComputePipeline,
    max_binding: u64,
    max_buffer: u64,
    /// GPU work is serialised: buffers are per-call, but this keeps memory pressure bounded.
    gate: Mutex<()>,
    errors: Arc<Mutex<Vec<String>>>,
}

struct Shared {
    _instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    dev: OnceLock<std::result::Result<Device, String>>,
}

/// Process-wide adapter (and lazily, device + pipelines), shared by every renderer.
fn shared() -> Option<&'static Shared> {
    static S: OnceLock<Option<Shared>> = OnceLock::new();
    S.get_or_init(|| {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .ok()?;
        if adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            return None;
        }
        Some(Shared {
            _instance: instance,
            adapter,
            dev: OnceLock::new(),
        })
    })
    .as_ref()
}

pub struct GpuRenderer {
    s: &'static Shared,
}

impl GpuRenderer {
    /// Probe for an adapter (cheap after the first call); the device and pipelines are
    /// created lazily on first use and shared process-wide.
    pub fn probe() -> Option<GpuRenderer> {
        shared().map(|s| GpuRenderer { s })
    }

    pub fn adapter_name(&self) -> String {
        self.s.adapter.get_info().name
    }

    fn device(&self) -> Result<&'static Device> {
        self.s
            .dev
            .get_or_init(|| Device::new(&self.s.adapter).map_err(|e| format!("{e:#}")))
            .as_ref()
            .map_err(|e| anyhow::anyhow!("gpu init failed: {e}"))
    }

    /// Render strictly on the GPU (errors instead of falling back).
    pub fn render_gpu_only(&self, req: &RenderRequest<'_>) -> Result<RgbImage> {
        let plan = prep::build(req)?;
        self.device()?.render(&plan, req.source)
    }
}

impl Renderer for GpuRenderer {
    fn backend(&self) -> Backend {
        Backend::Gpu
    }
    fn render(&self, req: &RenderRequest<'_>) -> Result<RgbImage> {
        let plan = prep::build(req)?;
        match self.device().and_then(|d| d.render(&plan, req.source)) {
            Ok(img) => Ok(img),
            Err(_) => Ok(cpu::render_plan(&plan, req.source)),
        }
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Device {
    fn new(adapter: &wgpu::Adapter) -> Result<Device> {
        let limits = adapter.limits();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("ip-render"),
                required_features: wgpu::Features::empty(),
                required_limits: limits.clone(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))?;
        let errors: Arc<Mutex<Vec<String>>> = Arc::default();
        {
            let errs = errors.clone();
            device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
                errs.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(e.to_string());
            }));
        }
        let main_entries = [
            uniform_entry(0),
            storage_entry(1, true),
            storage_entry(2, true),
            storage_entry(3, true),
            storage_entry(4, true),
            storage_entry(5, true),
            storage_entry(6, true),
            storage_entry(7, false),
        ];
        let layer_entries = [
            uniform_entry(0),
            storage_entry(1, true),
            storage_entry(2, false),
        ];
        let bgl_main = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("main"),
            entries: &main_entries,
        });
        let bgl_layer = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &layer_entries,
        });
        let sm_render = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render"),
            source: wgpu::ShaderSource::Wgsl(RENDER_WGSL.into()),
        });
        let sm_layers = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layers"),
            source: wgpu::ShaderSource::Wgsl(LAYERS_WGSL.into()),
        });
        let mk = |bgl: &wgpu::BindGroupLayout, module: &wgpu::ShaderModule, entry: &str| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(bgl)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let dev = Device {
            p_final: mk(&bgl_main, &sm_render, "final_main"),
            p_proxy: mk(&bgl_main, &sm_render, "proxy_main"),
            p_layer_a: mk(&bgl_layer, &sm_layers, "layer_a"),
            p_layer_b: mk(&bgl_layer, &sm_layers, "layer_b"),
            p_beauty_a: mk(&bgl_layer, &sm_layers, "beauty_a"),
            p_beauty_b: mk(&bgl_layer, &sm_layers, "beauty_b"),
            p_beauty_c: mk(&bgl_layer, &sm_layers, "beauty_c"),
            bgl_main,
            bgl_layer,
            max_binding: limits.max_storage_buffer_binding_size,
            max_buffer: limits.max_buffer_size,
            device,
            queue,
            gate: Mutex::new(()),
            errors,
        };
        dev.check_errors()?;
        Ok(dev)
    }

    fn check_errors(&self) -> Result<()> {
        let mut e = self.errors.lock().unwrap_or_else(|p| p.into_inner());
        if e.is_empty() {
            Ok(())
        } else {
            let msg = e.join("; ");
            e.clear();
            Err(anyhow::anyhow!("wgpu error: {msg}"))
        }
    }

    fn storage(&self, label: &str, bytes: &[u8], min: usize) -> wgpu::Buffer {
        let size = (bytes.len().max(min) + 3) & !3;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let head = bytes.len() & !3;
        if head > 0 {
            self.queue.write_buffer(&buf, 0, &bytes[..head]);
        }
        if head < bytes.len() {
            let mut tail = [0u8; 4];
            tail[..bytes.len() - head].copy_from_slice(&bytes[head..]);
            self.queue.write_buffer(&buf, head as u64, &tail);
        }
        buf
    }

    fn scratch(&self, label: &str, size: u64, extra: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (size.max(16) + 3) & !3,
            usage: wgpu::BufferUsages::STORAGE | extra,
            mapped_at_creation: false,
        })
    }

    fn uniform(&self, p: &Params) -> wgpu::Buffer {
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buf, 0, bytemuck::bytes_of(p));
        buf
    }

    fn render(&self, plan: &Plan, src: &RgbImage) -> Result<RgbImage> {
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        self.check_errors()?;
        let (ow, oh) = plan.out_size();
        anyhow::ensure!(
            src.data.len() as u64 <= self.max_binding.min(self.max_buffer),
            "source too large for this GPU ({} bytes)",
            src.data.len()
        );
        // ---- static buffers
        let src_buf = self.storage("src", &src.data, 16);
        let mut ops_f = Vec::with_capacity(plan.ops.len().max(1) * REC);
        for o in &plan.ops {
            ops_f.extend_from_slice(o);
        }
        if ops_f.is_empty() {
            ops_f.resize(REC, 0.0);
        }
        let ops_buf = self.storage("ops", bytemuck::cast_slice(&ops_f), 16);
        let proxy_px = plan.layer_len() as u64;
        let layers_bytes = plan.layer_slots as u64 * proxy_px * 16;
        anyhow::ensure!(
            layers_bytes <= self.max_binding,
            "layers exceed storage binding size"
        );
        let layers_buf = self.scratch("layers", layers_bytes, wgpu::BufferUsages::empty());
        // Planes = AI/skin planes, feature maps, blemishes, then the warp field.
        let warp_off = plan.planes.len() as u32;
        let mut planes_all: Vec<f32> =
            Vec::with_capacity(plan.planes.len() + plan.warp.as_ref().map_or(0, |w| w.data.len()));
        planes_all.extend_from_slice(&plan.planes);
        let (warp_w, warp_h, warp_on) = match &plan.warp {
            Some(w) => {
                planes_all.extend_from_slice(&w.data);
                (w.gw, w.gh, 1)
            }
            None => (0, 0, 0),
        };
        let planes_buf = self.storage("planes", bytemuck::cast_slice(&planes_all), 16);
        let mut tables = Vec::with_capacity(256 + plan.curves.len());
        tables.extend_from_slice(dec_lut());
        tables.extend_from_slice(&plan.curves);
        let tables_buf = self.storage("tables", bytemuck::cast_slice(&tables), 16);
        let mut lut_f: Vec<f32> = Vec::new();
        let mut lut_meta = [[0.0f32; 4]; 4];
        for (i, l) in plan.luts.iter().enumerate() {
            lut_meta[i] = [(lut_f.len() / 4) as f32, l.lut.size as f32, l.amount, 0.0];
            for e in &l.lut.data {
                lut_f.extend_from_slice(&[e[0], e[1], e[2], 0.0]);
            }
        }
        let luts_buf = self.storage("luts", bytemuck::cast_slice(&lut_f), 16);

        // ---- parameters common to all passes
        let base = |g: &Geo, nops: u32| -> Params {
            let (sig_hs, sig_cl, sig_d) = cpu::layer_sigmas(plan.proxy);
            Params {
                row0: [plan.src_w, plan.src_h, g.ow, g.oh],
                row1: [0, 0, g.ow, g.oh],
                geo0: [g.cx0, g.cy0, g.xs, g.ys],
                geo1: [g.cos, g.sin, g.hw, g.hh],
                row2: [g.taps, nops, plan.proxy.0, plan.proxy.1],
                row3: [plan.luts.len() as u32, 0, 0, 0],
                misc: [plan.sharpen, 0.0, 0.0, 0.0],
                luts: lut_meta,
                sig: [sig_hs, sig_cl, sig_d, 0.0],
                wrp: [warp_off, warp_w, warp_h, warp_on],
                rect: [0.0; 4],
            }
        };

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render"),
            });
        let main_bg = |p: &Params, out: &wgpu::Buffer| {
            let ub = self.uniform(p);
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("main"),
                layout: &self.bgl_main,
                entries: &[
                    bind(0, &ub),
                    bind(1, &src_buf),
                    bind(2, &ops_buf),
                    bind(3, &layers_buf),
                    bind(4, &planes_buf),
                    bind(5, &tables_buf),
                    bind(6, &luts_buf),
                    bind(7, out),
                ],
            })
        };
        let dispatch = |enc: &mut wgpu::CommandEncoder,
                        pipe: &wgpu::ComputePipeline,
                        bg: &wgpu::BindGroup,
                        w: u32,
                        h: u32| {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, bg, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        };

        // ---- neighbourhood layers
        if !plan.layer_ops.is_empty() {
            let (pw, ph) = plan.proxy;
            let pg = plan.proxy_geo();
            let proxy_buf = self.scratch("proxy", proxy_px * 16, wgpu::BufferUsages::empty());
            let tmp_buf = self.scratch("tmp", proxy_px * 16, wgpu::BufferUsages::empty());
            let lo_buf = self.scratch("lo", proxy_px * 16, wgpu::BufferUsages::empty());
            for &k in plan.layer_ops.iter() {
                let rec = &plan.ops[k];
                let mut p = base(&pg, k as u32);
                p.row1 = [0, 0, pw, ph];
                p.row3[1] = rec[prep::r::LAYER_OFF] as u32;
                let bg = main_bg(&p, &proxy_buf);
                dispatch(&mut enc, &self.p_proxy, &bg, pw, ph);
                let pass = |enc: &mut wgpu::CommandEncoder,
                            ub: &wgpu::Buffer,
                            pipe: &wgpu::ComputePipeline,
                            i: &wgpu::Buffer,
                            o: &wgpu::Buffer| {
                    let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("layer"),
                        layout: &self.bgl_layer,
                        entries: &[bind(0, ub), bind(1, i), bind(2, o)],
                    });
                    dispatch(enc, pipe, &bg, pw, ph);
                };
                if rec[prep::r::KIND] == prep::r::KIND_BEAUTY {
                    use prep::r;
                    p.sig = [
                        rec[r::B_SIGLO],
                        rec[r::B_RAD],
                        rec[r::B_RANGE],
                        (rec[r::B_RAD] * 0.5).max(1.0),
                    ];
                    p.rect = [
                        rec[r::B_RECT],
                        rec[r::B_RECT + 1],
                        rec[r::B_RECT + 2],
                        rec[r::B_RECT + 3],
                    ];
                    let ub = self.uniform(&p);
                    pass(&mut enc, &ub, &self.p_beauty_a, &proxy_buf, &tmp_buf);
                    pass(&mut enc, &ub, &self.p_beauty_b, &tmp_buf, &lo_buf);
                    pass(&mut enc, &ub, &self.p_beauty_c, &lo_buf, &layers_buf);
                } else {
                    let ub = self.uniform(&p);
                    pass(&mut enc, &ub, &self.p_layer_a, &proxy_buf, &tmp_buf);
                    pass(&mut enc, &ub, &self.p_layer_b, &tmp_buf, &layers_buf);
                }
            }
        }

        // ---- tiles
        let cap = (self.max_binding.min(self.max_buffer) / 4).min(MAX_TILE_PX);
        let rows_per_tile = ((cap / ow as u64).max(1) as u32).min(oh);
        let tile_px = ow as u64 * rows_per_tile as u64;
        let out_buf = self.scratch("tile_out", tile_px * 4, wgpu::BufferUsages::COPY_SRC);
        let read_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: tile_px * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut data = vec![0u8; ow as usize * oh as usize * 3];
        let mut y0 = 0u32;
        while y0 < oh {
            let th = rows_per_tile.min(oh - y0);
            let mut p = base(&plan.geo, plan.ops.len() as u32);
            p.row1 = [0, y0, ow, th];
            let bg = main_bg(&p, &out_buf);
            dispatch(&mut enc, &self.p_final, &bg, ow, th);
            let bytes = ow as u64 * th as u64 * 4;
            enc.copy_buffer_to_buffer(&out_buf, 0, &read_buf, 0, bytes);
            self.queue.submit(Some(enc.finish()));
            let slice = read_buf.slice(..bytes);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.device.poll(wgpu::PollType::wait_indefinitely())?;
            self.check_errors()?;
            rx.recv()??;
            {
                let view = slice.get_mapped_range()?;
                let px: &[u8] = &view;
                for (i, c) in px.chunks_exact(4).enumerate() {
                    let o = (y0 as usize * ow as usize + i) * 3;
                    data[o] = c[0];
                    data[o + 1] = c[1];
                    data[o + 2] = c[2];
                }
            }
            read_buf.unmap();
            y0 += th;
            enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("tile"),
                });
        }
        Ok(RgbImage {
            width: ow,
            height: oh,
            data,
        })
    }
}

fn bind(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buf.as_entire_binding(),
    }
}

/// GPU renderer that reports errors instead of falling back to the CPU (tests, diagnostics).
pub struct StrictGpu(pub GpuRenderer);

impl Renderer for StrictGpu {
    fn backend(&self) -> Backend {
        Backend::Gpu
    }
    fn render(&self, req: &RenderRequest<'_>) -> Result<RgbImage> {
        self.0.render_gpu_only(req)
    }
}
