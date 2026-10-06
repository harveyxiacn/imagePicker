// Neighbourhood layers on the low-resolution proxy grid (port of `layers_from` in cpu.rs).
// layer_a: proxy image (linear rgb) -> horizontal pass; layer_b: vertical pass -> layers.

struct Params {
    row0: vec4<u32>,   // _, _, out_w, out_h (unused here)
    row1: vec4<u32>,
    geo0: vec4<f32>,
    geo1: vec4<f32>,
    row2: vec4<u32>,   // taps, nops, proxy_w, proxy_h
    row3: vec4<u32>,   // nluts, layer_off (vec4 units), mode, _
    misc: vec4<f32>,
    luts: array<vec4<f32>, 4>,
    sig: vec4<f32>,    // sigmas hs, cl, d | beauty: sigma_lo, radius, range sigma, spatial sigma
    wrp: vec4<u32>,
    rect: vec4<f32>,   // beauty layer rect (normalised x0, y0, x1, y1)
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read> inb: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> outb: array<vec4<f32>>;

fn lum(c: vec3<f32>) -> f32 {
    return c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722;
}

fn radius(s: f32) -> i32 {
    return i32(ceil(3.0 * s));
}

@compute @workgroup_size(8, 8, 1)
fn layer_a(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.row2.z);
    let h = i32(P.row2.w);
    if gid.x >= u32(w) || gid.y >= u32(h) {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let row = y * w;
    var res = vec4<f32>(0.0);
    for (var ch = 0u; ch < 3u; ch = ch + 1u) {
        let s = P.sig[ch];
        let r = radius(s);
        var acc = 0.0;
        var ws = 0.0;
        for (var d = -r; d <= r; d = d + 1) {
            let wgt = exp(-f32(d * d) / (2.0 * s * s));
            let xi = clamp(x + d, 0, w - 1);
            var v: f32;
            if ch < 2u {
                v = lum(inb[row + xi].xyz);
            } else {
                v = 1e30;
                for (var j = -2; j <= 2; j = j + 1) {
                    let xj = clamp(xi + j, 0, w - 1);
                    let c = inb[row + xj].xyz;
                    v = min(v, min(c.x, min(c.y, c.z)));
                }
            }
            acc = acc + wgt * v;
            ws = ws + wgt;
        }
        res[ch] = acc / ws;
    }
    outb[row + x] = res;
}

@compute @workgroup_size(8, 8, 1)
fn layer_b(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.row2.z);
    let h = i32(P.row2.w);
    if gid.x >= u32(w) || gid.y >= u32(h) {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    var res = vec4<f32>(0.0);
    for (var ch = 0u; ch < 3u; ch = ch + 1u) {
        let s = P.sig[ch];
        let r = radius(s);
        var acc = 0.0;
        var ws = 0.0;
        for (var d = -r; d <= r; d = d + 1) {
            let wgt = exp(-f32(d * d) / (2.0 * s * s));
            let yi = clamp(y + d, 0, h - 1);
            var v: f32;
            if ch < 2u {
                v = inb[yi * w + x][ch];
            } else {
                v = 1e30;
                for (var j = -2; j <= 2; j = j + 1) {
                    let yj = clamp(yi + j, 0, h - 1);
                    v = min(v, inb[yj * w + x].z);
                }
            }
            acc = acc + wgt * v;
            ws = ws + wgt;
        }
        res[ch] = acc / ws;
    }
    outb[P.row3.y + u32(y * w + x)] = res;
}

// ---------------------------------------------------------------- beauty (frequency separation)
// Port of `beauty_layers` in cpu.rs. beauty_a: horizontal Gaussian of the sRGB-encoded proxy;
// beauty_b: vertical Gaussian -> Lo; beauty_c: bilateral over Lo inside the rect, writes both
// layer slots (Lo at layer_off, Ls at layer_off + w * h).

fn enc1(x0: f32) -> f32 {
    let x = clamp(x0, 0.0, 1.0);
    if x <= 0.0031308 {
        return x * 12.92;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

fn enc3(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(enc1(c.x), enc1(c.y), enc1(c.z));
}

@compute @workgroup_size(8, 8, 1)
fn beauty_a(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.row2.z);
    let h = i32(P.row2.w);
    if gid.x >= u32(w) || gid.y >= u32(h) {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let s = P.sig.x;
    let r = i32(ceil(3.0 * s));
    var acc = vec3<f32>(0.0);
    var ws = 0.0;
    for (var d = -r; d <= r; d = d + 1) {
        let wgt = exp(-f32(d * d) / (2.0 * s * s));
        let xi = clamp(x + d, 0, w - 1);
        acc = acc + wgt * enc3(inb[y * w + xi].xyz);
        ws = ws + wgt;
    }
    outb[y * w + x] = vec4<f32>(acc / ws, 0.0);
}

@compute @workgroup_size(8, 8, 1)
fn beauty_b(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.row2.z);
    let h = i32(P.row2.w);
    if gid.x >= u32(w) || gid.y >= u32(h) {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let s = P.sig.x;
    let r = i32(ceil(3.0 * s));
    var acc = vec3<f32>(0.0);
    var ws = 0.0;
    for (var d = -r; d <= r; d = d + 1) {
        let wgt = exp(-f32(d * d) / (2.0 * s * s));
        let yi = clamp(y + d, 0, h - 1);
        acc = acc + wgt * inb[yi * w + x].xyz;
        ws = ws + wgt;
    }
    outb[y * w + x] = vec4<f32>(acc / ws, 0.0);
}

@compute @workgroup_size(8, 8, 1)
fn beauty_c(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.row2.z);
    let h = i32(P.row2.w);
    if gid.x >= u32(w) || gid.y >= u32(h) {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let c = inb[y * w + x].xyz;
    let off = P.row3.y;
    let px = u32(w * h);
    let i = u32(y * w + x);
    outb[off + i] = vec4<f32>(c, 0.0);
    let x0 = min(u32(floor(P.rect.x * f32(w))), u32(w));
    let x1 = min(u32(ceil(P.rect.z * f32(w))), u32(w));
    let y0 = min(u32(floor(P.rect.y * f32(h))), u32(h));
    let y1 = min(u32(ceil(P.rect.w * f32(h))), u32(h));
    if gid.x < x0 || gid.x >= x1 || gid.y < y0 || gid.y >= y1 {
        outb[off + px + i] = vec4<f32>(c, 0.0);
        return;
    }
    let rad = i32(P.sig.y);
    let sr = P.sig.z;
    let ss_ = P.sig.w;
    var acc = vec3<f32>(0.0);
    var ws = 0.0;
    for (var dy = -rad; dy <= rad; dy = dy + 1) {
        let yi = y + dy;
        if yi < 0 || yi >= h {
            continue;
        }
        for (var dx = -rad; dx <= rad; dx = dx + 1) {
            let xi = x + dx;
            if xi < 0 || xi >= w {
                continue;
            }
            let n = inb[yi * w + xi].xyz;
            let dd = n - c;
            let d2 = dd.x * dd.x + dd.y * dd.y + dd.z * dd.z;
            let wgt = exp(-f32(dx * dx + dy * dy) / (2.0 * ss_ * ss_)) * exp(-d2 / (2.0 * sr * sr));
            acc = acc + wgt * n;
            ws = ws + wgt;
        }
    }
    outb[off + px + i] = vec4<f32>(acc / ws, 0.0);
}
