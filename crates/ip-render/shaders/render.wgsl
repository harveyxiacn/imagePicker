// imagePicker render pipeline (GPU). Line-for-line port of crates/ip-render/src/cpu.rs;
// op record layout (REC = 64 floats) is defined in src/prep.rs (module `r`).

struct Params {
    row0: vec4<u32>,   // src_w, src_h, out_w, out_h   (grid dimensions)
    row1: vec4<u32>,   // tile_x, tile_y, tile_w, tile_h
    geo0: vec4<f32>,   // cx0, cy0, xs, ys
    geo1: vec4<f32>,   // cos, sin, hw, hh
    row2: vec4<u32>,   // taps, nops, proxy_w, proxy_h
    row3: vec4<u32>,   // nluts, layer_off (vec4 units), mode, _
    misc: vec4<f32>,   // sharpen, _, _, _
    luts: array<vec4<f32>, 4>, // offset (vec4 units), size, amount, _
    sig: vec4<f32>,    // layer sigmas (hs, cl, d)
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> ops: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> layers: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> planes: array<f32>;
@group(0) @binding(5) var<storage, read> tables: array<f32>; // [0..256) sRGB decode, then curves
@group(0) @binding(6) var<storage, read> luts: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> outbuf: array<u32>;

const CURVE_BASE: u32 = 256u;
const CURVE_N: u32 = 1024u;
const HAZE_A: f32 = 0.92;
const SKIN_HUE: f32 = 55.0;
const MID_PV: f32 = 0.4614;
const PI: f32 = 3.14159265358979;

var<private> HC: array<f32, 8> = array<f32, 8>(29.0, 55.0, 110.0, 142.0, 195.0, 264.0, 300.0, 330.0);

// ------------------------------------------------------------------ colour

fn lum(c: vec3<f32>) -> f32 {
    return c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722;
}

fn enc1(x0: f32) -> f32 {
    let x = clamp(x0, 0.0, 1.0);
    if x <= 0.0031308 {
        return x * 12.92;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

fn dec1(x: f32) -> f32 {
    if x <= 0.04045 {
        return x / 12.92;
    }
    return pow((x + 0.055) / 1.055, 2.4);
}

fn enc3(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(enc1(c.x), enc1(c.y), enc1(c.z));
}

fn ss(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn cbrt_pos(x: f32) -> f32 {
    if x > 1e-12 {
        return pow(x, 1.0 / 3.0);
    }
    return 0.0;
}

fn to_oklab(c: vec3<f32>) -> vec3<f32> {
    let l = 0.4122214708 * c.x + 0.5363325363 * c.y + 0.0514459929 * c.z;
    let m = 0.2119034982 * c.x + 0.6806995451 * c.y + 0.1073969566 * c.z;
    let s = 0.0883024619 * c.x + 0.2817188376 * c.y + 0.6299787005 * c.z;
    let l_ = cbrt_pos(l);
    let m_ = cbrt_pos(m);
    let s_ = cbrt_pos(s);
    return vec3<f32>(
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    );
}

fn from_oklab(c: vec3<f32>) -> vec3<f32> {
    let l_ = c.x + 0.3963377774 * c.y + 0.2158037573 * c.z;
    let m_ = c.x - 0.1055613458 * c.y - 0.0638541728 * c.z;
    let s_ = c.x - 0.0894841775 * c.y - 1.2914855480 * c.z;
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;
    return vec3<f32>(
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    );
}

// ------------------------------------------------------------------ geometry

fn src_byte(n: u32) -> u32 {
    return (src[n >> 2u] >> ((n & 3u) * 8u)) & 255u;
}

fn texel(x: i32, y: i32) -> vec3<f32> {
    let xi = u32(clamp(x, 0, i32(P.row0.x) - 1));
    let yi = u32(clamp(y, 0, i32(P.row0.y) - 1));
    let i = (yi * P.row0.x + xi) * 3u;
    return vec3<f32>(tables[src_byte(i)], tables[src_byte(i + 1u)], tables[src_byte(i + 2u)]);
}

fn bilinear(sx: f32, sy: f32) -> vec3<f32> {
    let x = sx - 0.5;
    let y = sy - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = texel(xi, yi);
    let b = texel(xi + 1, yi);
    let c = texel(xi, yi + 1);
    let d = texel(xi + 1, yi + 1);
    let top = a + (b - a) * fx;
    let bot = c + (d - c) * fx;
    return top + (bot - top) * fy;
}

fn geo_map(fx: f32, fy: f32) -> vec2<f32> {
    let cx = P.geo0.x + fx * P.geo0.z;
    let cy = P.geo0.y + fy * P.geo0.w;
    let dx = cx - P.geo1.z;
    let dy = cy - P.geo1.w;
    return vec2<f32>(
        P.geo1.z + dx * P.geo1.x - dy * P.geo1.y,
        P.geo1.w + dx * P.geo1.y + dy * P.geo1.x,
    );
}

fn geo_sample(ox: u32, oy: u32) -> vec3<f32> {
    let fx = f32(ox) + 0.5;
    let fy = f32(oy) + 0.5;
    let n = P.row2.x;
    if n == 1u {
        let s = geo_map(fx, fy);
        return bilinear(s.x, s.y);
    }
    let inv = 1.0 / f32(n);
    var acc = vec3<f32>(0.0);
    for (var j = 0u; j < n; j = j + 1u) {
        let oyy = fy + ((f32(j) + 0.5) * inv - 0.5);
        for (var i = 0u; i < n; i = i + 1u) {
            let oxx = fx + ((f32(i) + 0.5) * inv - 0.5);
            let s = geo_map(oxx, oyy);
            acc = acc + bilinear(s.x, s.y);
        }
    }
    return acc * (inv * inv);
}

// ------------------------------------------------------------------ op records

fn R(k: u32, i: u32) -> f32 {
    let v = ops[k * 16u + (i >> 2u)];
    return v[i & 3u];
}

fn has(k: u32, f: u32) -> bool {
    return (u32(R(k, 1u)) & f) != 0u;
}

fn layer_at(off: u32, u: f32, v: f32) -> vec4<f32> {
    let pw = P.row2.z;
    let ph = P.row2.w;
    let x = u * f32(pw) - 0.5;
    let y = v * f32(ph) - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let xa = u32(clamp(i32(x0), 0, i32(pw) - 1));
    let xb = u32(clamp(i32(x0) + 1, 0, i32(pw) - 1));
    let ya = u32(clamp(i32(y0), 0, i32(ph) - 1));
    let yb = u32(clamp(i32(y0) + 1, 0, i32(ph) - 1));
    let a = layers[off + ya * pw + xa];
    let b = layers[off + ya * pw + xb];
    let c = layers[off + yb * pw + xa];
    let d = layers[off + yb * pw + xb];
    let t = a + (b - a) * fx;
    let bt = c + (d - c) * fx;
    return t + (bt - t) * fy;
}

fn mask_value(k: u32, u: f32, v: f32, guide: f32) -> f32 {
    let kind = R(k, 0u);
    var m: f32;
    if kind == 1.0 {
        let off = u32(R(k, 28u));
        let pw = u32(R(k, 29u));
        let ph = u32(R(k, 30u));
        let x = u * f32(pw) - 0.5;
        let y = v * f32(ph) - 0.5;
        let x0 = floor(x);
        let y0 = floor(y);
        let fx = x - x0;
        let fy = y - y0;
        let xa = u32(clamp(i32(x0), 0, i32(pw) - 1));
        let xb = u32(clamp(i32(x0) + 1, 0, i32(pw) - 1));
        let ya = u32(clamp(i32(y0), 0, i32(ph) - 1));
        let yb = u32(clamp(i32(y0) + 1, 0, i32(ph) - 1));
        var ab = vec2<f32>(0.0);
        for (var c = 0u; c < 2u; c = c + 1u) {
            let p00 = planes[off + (ya * pw + xa) * 2u + c];
            let p10 = planes[off + (ya * pw + xb) * 2u + c];
            let p01 = planes[off + (yb * pw + xa) * 2u + c];
            let p11 = planes[off + (yb * pw + xb) * 2u + c];
            let t = p00 + (p10 - p00) * fx;
            let b = p01 + (p11 - p01) * fx;
            ab[c] = t + (b - t) * fy;
        }
        m = clamp(ab.x * guide + ab.y, 0.0, 1.0);
    } else if kind == 2.0 {
        let dx = (u - R(k, 28u)) / R(k, 30u);
        let dy = (v - R(k, 29u)) / R(k, 31u);
        let d = sqrt(dx * dx + dy * dy);
        m = 1.0 - ss(1.0 - R(k, 32u), 1.0, d);
    } else {
        let sx = R(k, 28u);
        let sy = R(k, 29u);
        let ex = R(k, 30u) - sx;
        let ey = R(k, 31u) - sy;
        let len2 = max(ex * ex + ey * ey, 1e-8);
        let t = ((u - sx) * ex + (v - sy) * ey) / len2;
        m = 1.0 - ss(0.0, 1.0, t);
    }
    if R(k, 3u) > 0.5 {
        m = 1.0 - m;
    }
    return m * R(k, 2u);
}

// ------------------------------------------------------------------ adjust

fn adjust(k: u32, rgb_in: vec3<f32>, lay: vec4<f32>) -> vec3<f32> {
    var c = rgb_in;
    if has(k, 1u) {
        c = max(vec3<f32>(
            R(k, 4u) * c.x + R(k, 5u) * c.y + R(k, 6u) * c.z,
            R(k, 7u) * c.x + R(k, 8u) * c.y + R(k, 9u) * c.z,
            R(k, 10u) * c.x + R(k, 11u) * c.y + R(k, 12u) * c.z,
        ), vec3<f32>(0.0));
    }
    if has(k, 2u) {
        c = c * R(k, 13u);
        if R(k, 15u) > 0.0 {
            let knee = R(k, 14u);
            let m = max(c.x, max(c.y, c.z));
            if m > knee {
                let m2 = knee + (1.0 - knee) * (1.0 - exp(-(m - knee) / (1.0 - knee)));
                c = c * (m2 / m);
            }
        }
    }
    if has(k, 4u) {
        let lb = lay.x * R(k, 13u);
        let p = enc1(lb);
        let ws = 1.0 - ss(0.1, 0.65, p);
        let wh = ss(0.35, 0.9, p);
        c = c * exp2(R(k, 16u) * ws + R(k, 17u) * wh);
    }
    if has(k, 8u) {
        var py = enc1(lum(c));
        c = c * exp2(R(k, 18u) * ss(0.4, 1.0, py));
        py = enc1(lum(c));
        let d = R(k, 19u) * (1.0 - ss(0.0, 0.35, py));
        c = max(c + vec3<f32>(d), vec3<f32>(0.0));
    }
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    if has(k, 16u) {
        let e = R(k, 20u);
        for (var i = 0u; i < 3u; i = i + 1u) {
            let p = enc1(c[i]);
            var q: f32;
            if p < MID_PV {
                q = MID_PV * pow(p / MID_PV, e);
            } else {
                q = 1.0 - (1.0 - MID_PV) * pow((1.0 - p) / (1.0 - MID_PV), e);
            }
            c[i] = dec1(clamp(q, 0.0, 1.0));
        }
    }
    if has(k, 32u) {
        let yin = lum(rgb_in);
        let ratio = clamp(yin / max(lay.y, 1e-4), 0.25, 4.0);
        let pm = enc1(lum(c));
        let wm = 1.0 - (2.0 * pm - 1.0) * (2.0 * pm - 1.0);
        c = c * exp2(log2(ratio) * R(k, 21u) * wm);
    }
    if has(k, 64u) {
        let w = R(k, 22u);
        if w > 0.0 {
            let t = clamp(1.0 - w * lay.z / HAZE_A, 0.25, 1.0);
            c = (c - vec3<f32>(HAZE_A)) / t + vec3<f32>(HAZE_A);
        }
        let s = R(k, 23u);
        if s > 0.0 {
            c = c * (1.0 - s) + vec3<f32>(HAZE_A * s);
        }
        c = max(c, vec3<f32>(0.0));
    }
    if has(k, 128u) {
        let off = CURVE_BASE + u32(R(k, 26u));
        for (var i = 0u; i < 3u; i = i + 1u) {
            let p = enc1(c[i]) * f32(CURVE_N - 1u);
            let idx = min(u32(floor(p)), CURVE_N - 2u);
            let f = p - f32(idx);
            let base = off + i * CURVE_N + idx;
            c[i] = dec1(tables[base] + (tables[base + 1u] - tables[base]) * f);
        }
    }
    if (u32(R(k, 1u)) & (256u | 512u | 1024u)) != 0u {
        var lab = to_oklab(c);
        let chroma = length(vec2<f32>(lab.y, lab.z));
        var hue = 0.0;
        if chroma > 1e-6 {
            hue = atan2(lab.z, lab.y);
        }
        var cs = 1.0;
        if has(k, 256u) {
            let wv = 1.0 - clamp(chroma / 0.26, 0.0, 1.0);
            var dh = abs(hue * 180.0 / PI - SKIN_HUE) % 360.0;
            if dh > 180.0 {
                dh = 360.0 - dh;
            }
            let skin = 1.0 - 0.65 * exp(-(dh / 30.0) * (dh / 30.0));
            cs = cs * max(1.0 + R(k, 24u) * wv * skin, 0.0);
            cs = cs * max(1.0 + R(k, 25u), 0.0);
        }
        var dl = 0.0;
        if has(k, 512u) {
            var hd = hue * 180.0 / PI;
            if hd < 0.0 {
                hd = hd + 360.0;
            }
            let cw = ss(0.0, 0.03, chroma);
            var sh = 0.0;
            var sa = 0.0;
            var sl = 0.0;
            var hh = hd;
            if hh < HC[0] {
                hh = hh + 360.0;
            }
            var i0 = 7u;
            var i1 = 0u;
            var w1 = 0.0;
            for (var i = 0u; i < 8u; i = i + 1u) {
                let lo = HC[i];
                var hi = 389.0;
                if i < 7u {
                    hi = HC[i + 1u];
                }
                if hh >= lo && hh < hi {
                    let t = (hh - lo) / (hi - lo);
                    let sn = sin(PI * 0.5 * t);
                    i0 = i;
                    i1 = (i + 1u) % 8u;
                    w1 = sn * sn;
                    break;
                }
            }
            let w0 = 1.0 - w1;
            sh = w0 * R(k, 40u + i0 * 3u) + w1 * R(k, 40u + i1 * 3u);
            sa = w0 * R(k, 41u + i0 * 3u) + w1 * R(k, 41u + i1 * 3u);
            sl = w0 * R(k, 42u + i0 * 3u) + w1 * R(k, 42u + i1 * 3u);
            hue = hue + (sh * cw * 30.0) * PI / 180.0;
            cs = cs * max(1.0 + sa * cw, 0.0);
            dl = sl * cw * 0.25;
        }
        let nc = chroma * cs;
        lab.x = clamp(lab.x + dl, 0.0, 1.0);
        lab.y = nc * cos(hue);
        lab.z = nc * sin(hue);
        if has(k, 1024u) {
            let pivot = R(k, 33u);
            let l = lab.x;
            let ws = 1.0 - ss(0.0, pivot, l);
            let wh = ss(pivot, 1.0, l);
            let wm = 1.0 - ws - wh;
            lab.y = lab.y + ws * R(k, 34u) + wm * R(k, 36u) + wh * R(k, 38u);
            lab.z = lab.z + ws * R(k, 35u) + wm * R(k, 37u) + wh * R(k, 39u);
        }
        c = max(from_oklab(lab), vec3<f32>(0.0));
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn eval_ops(rgb0: vec3<f32>, u: f32, v: f32, nops: u32) -> vec3<f32> {
    let guide = enc1(lum(rgb0));
    var c = rgb0;
    for (var k = 0u; k < nops; k = k + 1u) {
        let global = R(k, 0u) == 0.0;
        var m = 1.0;
        if !global {
            m = mask_value(k, u, v, guide);
        }
        if m <= 0.0 {
            continue;
        }
        var lay = vec4<f32>(0.0);
        if (u32(R(k, 1u)) & (4u | 32u | 64u)) != 0u {
            lay = layer_at(u32(R(k, 27u)), u, v);
        }
        let a = adjust(k, c, lay);
        if global {
            c = a;
        } else {
            c = c + (a - c) * m;
        }
    }
    return c;
}

fn lut_apply(off: u32, n: u32, e: vec3<f32>) -> vec3<f32> {
    let nm = f32(n - 1u);
    var idx = vec3<u32>(0u);
    var fr = vec3<f32>(0.0);
    for (var k = 0u; k < 3u; k = k + 1u) {
        let p = clamp(e[k], 0.0, 1.0) * nm;
        let i = min(u32(floor(p)), n - 2u);
        idx[k] = i;
        fr[k] = p - f32(i);
    }
    let b0 = off + idx.x + idx.y * n + idx.z * n * n;
    let c000 = luts[b0].xyz;
    let c100 = luts[b0 + 1u].xyz;
    let c010 = luts[b0 + n].xyz;
    let c110 = luts[b0 + n + 1u].xyz;
    let c001 = luts[b0 + n * n].xyz;
    let c101 = luts[b0 + n * n + 1u].xyz;
    let c011 = luts[b0 + n * n + n].xyz;
    let c111 = luts[b0 + n * n + n + 1u].xyz;
    let c00 = c000 + (c100 - c000) * fr.x;
    let c10 = c010 + (c110 - c010) * fr.x;
    let c01 = c001 + (c101 - c001) * fr.x;
    let c11 = c011 + (c111 - c011) * fr.x;
    let c0 = c00 + (c10 - c00) * fr.y;
    let c1 = c01 + (c11 - c01) * fr.y;
    return c0 + (c1 - c0) * fr.z;
}

// Linear pixel after ops at a grid position.
fn lin_px(ox: u32, oy: u32, nops: u32) -> vec3<f32> {
    let c0 = geo_sample(ox, oy);
    let u = (f32(ox) + 0.5) / f32(P.row0.z);
    let v = (f32(oy) + 0.5) / f32(P.row0.w);
    return eval_ops(c0, u, v, nops);
}

// Encoded pixel (LUTs applied).
fn enc_px(ox: u32, oy: u32) -> vec3<f32> {
    let c = lin_px(ox, oy, P.row2.y);
    var e = enc3(c);
    for (var i = 0u; i < P.row3.x; i = i + 1u) {
        let l = P.luts[i];
        let o = lut_apply(u32(l.x), u32(l.y), e);
        e = e + (o - e) * l.z;
    }
    return e;
}

fn to8(v: f32) -> u32 {
    return u32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
}

@compute @workgroup_size(8, 8, 1)
fn final_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= P.row1.z || gid.y >= P.row1.w {
        return;
    }
    let ox = P.row1.x + gid.x;
    let oy = P.row1.y + gid.y;
    let c = enc_px(ox, oy);
    var o = c;
    if P.misc.x > 0.0 {
        let xl = max(ox, 1u) - 1u;
        let xr = min(ox + 1u, P.row0.z - 1u);
        let yu = max(oy, 1u) - 1u;
        let yd = min(oy + 1u, P.row0.w - 1u);
        let l = enc_px(xl, oy);
        let r = enc_px(xr, oy);
        let u = enc_px(ox, yu);
        let d = enc_px(ox, yd);
        let yc = lum(c);
        let blur = (4.0 * yc + lum(l) + lum(r) + lum(u) + lum(d)) / 8.0;
        let delta = P.misc.x / 100.0 * 1.6 * (yc - blur);
        o = clamp(c + vec3<f32>(delta), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    outbuf[gid.y * P.row1.z + gid.x] = to8(o.x) | (to8(o.y) << 8u) | (to8(o.z) << 16u) | (255u << 24u);
}

@compute @workgroup_size(8, 8, 1)
fn proxy_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= P.row0.z || gid.y >= P.row0.w {
        return;
    }
    let c = lin_px(gid.x, gid.y, P.row2.y);
    let i = (gid.y * P.row0.z + gid.x) * 4u;
    outbuf[i] = bitcast<u32>(c.x);
    outbuf[i + 1u] = bitcast<u32>(c.y);
    outbuf[i + 2u] = bitcast<u32>(c.z);
    outbuf[i + 3u] = 0u;
}
