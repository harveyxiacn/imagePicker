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
    sig: vec4<f32>,    // sigmas hs, cl, d
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
