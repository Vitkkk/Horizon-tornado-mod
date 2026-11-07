// Stormfront particles: camera-facing billboards (puffs, chunks, leaves, pellets), rain
// streaks along their velocity, tall rain-shaft sheets and lines (lightning, overlays), drawn
// after the world with premultiplied alpha and faded softly against the world's depth.

struct Uniforms {
    // camera-relative world -> clip, without TAA jitter
    view_proj: mat4x4<f32>,
    // xyz: the frame's anchor minus the camera; w: seconds since the frame was built
    anchor_rel: vec4<f32>,
    // camera right and up (world), w unused
    right: vec4<f32>,
    up: vec4<f32>,
    // x: light multiplier, y: near plane, zw: depth texture size / colour target size
    params: vec4<f32>,
    // x, y: colour target size; z: 1 when the world's depth is bound; w: soft-fade distance
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var depth_tex: texture_depth_2d;

struct VsIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3<f32>,
    @location(1) size: f32,
    @location(2) vel: vec3<f32>,
    @location(3) color: vec4<f32>,
    @location(4) shape: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) shape: u32,
    @location(3) dist: f32,
};

fn corner(i: u32) -> vec2<f32> {
    switch i {
        case 0u: { return vec2<f32>(-1.0, -1.0); }
        case 1u: { return vec2<f32>(1.0, -1.0); }
        case 2u: { return vec2<f32>(1.0, 1.0); }
        case 3u: { return vec2<f32>(-1.0, -1.0); }
        case 4u: { return vec2<f32>(1.0, 1.0); }
        default: { return vec2<f32>(-1.0, 1.0); }
    }
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let kind = in.shape & 255u;
    let seed = f32((in.shape >> 8u) & 255u) / 255.0;
    let c = corner(in.vi);
    var center = in.pos + u.anchor_rel.xyz;
    var world: vec3<f32>;
    if kind == 5u {
        // a line from pos to pos + vel, `size` wide, facing the camera
        let a = center;
        let b = center + in.vel;
        let dir = normalize(in.vel + vec3<f32>(1e-5, 1e-5, 1e-5));
        let to_cam = normalize(-(a + b) * 0.5 + vec3<f32>(1e-5, 0.0, 0.0));
        let side = normalize(cross(dir, to_cam) + vec3<f32>(1e-6, 0.0, 0.0)) * in.size;
        world = mix(a, b, c.y * 0.5 + 0.5) + side * c.x;
    } else {
        let t = u.anchor_rel.w;
        center = center + in.vel * t;
        var r = u.right.xyz;
        var up = u.up.xyz;
        var sx = in.size;
        var sy = in.size;
        let to_cam = normalize(-center + vec3<f32>(1e-5, 0.0, 0.0));
        if kind == 2u {
            // a streak along the velocity
            up = normalize(in.vel + vec3<f32>(0.0, -1e-3, 0.0));
            r = normalize(cross(up, to_cam) + vec3<f32>(1e-6, 0.0, 0.0));
            sx = max(in.size, 0.02);
            sy = clamp(length(in.vel) * 0.06, 0.3, 2.5);
        } else if kind == 6u {
            // an upright sheet, four times as tall as wide
            up = vec3<f32>(0.0, 1.0, 0.0);
            r = normalize(cross(up, to_cam) + vec3<f32>(1e-6, 0.0, 0.0));
            sy = in.size * 4.0;
        } else {
            // turn each billboard by its own angle, tumbling chunks and leaves faster
            var spin = (seed - 0.5) * 0.6;
            if kind == 1u || kind == 4u {
                spin = (seed - 0.5) * 9.0;
            }
            let ang = seed * 6.2831853 + t * spin;
            let cs = cos(ang);
            let sn = sin(ang);
            let r2 = r * cs + up * sn;
            let u2 = up * cs - r * sn;
            r = r2;
            up = u2;
        }
        world = center + r * (c.x * sx) + up * (c.y * sy);
    }
    out.clip = u.view_proj * vec4<f32>(world, 1.0);
    out.uv = c;
    out.color = in.color;
    out.shape = in.shape;
    out.dist = out.clip.w;
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let kind = in.shape & 255u;
    let seed = f32((in.shape >> 8u) & 255u);
    let r = length(in.uv);
    var a = in.color.a;
    var rgb = in.color.rgb;
    switch kind {
        case 0u: {
            // a soft, lumpy puff, lit a little from above
            let n = noise(in.uv * 2.3 + vec2<f32>(seed * 0.37, seed * 0.11)) * 0.6 + noise(in.uv * 5.1 + vec2<f32>(seed)) * 0.4;
            a = a * smoothstep(1.0, 0.2, r + (n - 0.5) * 0.6);
            rgb = rgb * (0.85 + 0.25 * (in.uv.y * 0.5 + 0.5)) * (0.9 + 0.2 * n);
        }
        case 1u: {
            let edge = 0.65 + 0.3 * hash(vec2<f32>(seed, 3.0));
            a = a * step(max(abs(in.uv.x), abs(in.uv.y * 0.8)), edge);
            rgb = rgb * (0.8 + 0.3 * hash(floor(in.uv * 3.0) + vec2<f32>(seed)));
        }
        case 2u: {
            a = a * (1.0 - abs(in.uv.x)) * smoothstep(1.0, 0.5, abs(in.uv.y));
        }
        case 3u: {
            a = a * step(r, 1.0);
            rgb = rgb * (1.1 - 0.3 * r);
        }
        case 4u: {
            a = a * step(abs(in.uv.x) * 1.7 + abs(in.uv.y) * 0.9, 1.0);
        }
        case 5u: {
            a = a * (1.0 - abs(in.uv.x));
            rgb = rgb * 1.6;
        }
        case 6u: {
            let n = noise(vec2<f32>(in.uv.x * 5.0 + seed, in.uv.y * 0.7));
            a = a * smoothstep(1.0, 0.35, abs(in.uv.x)) * smoothstep(1.0, 0.55, in.uv.y) * smoothstep(-1.0, -0.75, in.uv.y) * (0.55 + 0.45 * n);
        }
        default: {}
    }
    if u.screen.z > 0.5 {
        // soft particles: fade where the world is just behind, hide where it is in front
        let px = vec2<i32>(in.clip.xy * u.params.zw);
        let scene = textureLoad(depth_tex, px, 0);
        let scene_dist = u.params.y / max(scene, 1e-7);
        let fade = clamp((scene_dist - in.dist) / max(u.screen.w, 0.01), 0.0, 1.0);
        a = a * fade;
    }
    if a < 0.003 {
        discard;
    }
    rgb = rgb * u.params.x;
    return vec4<f32>(rgb * a, a);
}
