
// for now lazy path - just split whole 1024x512 drawing area, than think of something...
// but! we need to implement scissors anyway...
struct Uniforms {
    drawing_area_left: i32,
    drawing_area_top: i32,
    drawing_area_bottom: i32,
    drawing_area_right: i32,
    num_vertices: u32,
};

struct Vertex {
    pos: u32,
    color: u32,
    uv: u32,
    clut: u32,
    texture_window: u32,
    texpage_base_and_flags: u32,
};

struct PartialDerivative {
    r_dx: i32,
    g_dx: i32,
    b_dx: i32,
    r_dy: i32,
    g_dy: i32,
    b_dy: i32,
}

fn get_pos(v: Vertex) -> vec2<i32> {
    let _x = v.pos & 0xFFFF;
    let _y = (v.pos >> 16) & 0xFFFF;
    let x = (_x & 0x7FFF) - (_x & 0x8000);
    let y = (_y & 0x7FFF) - (_y & 0x8000);
    return vec2(i32(x), i32(y));
    // return vec2<i32>((i32(v.pos & 0xFFFF) << 16) >> 16, ((i32((v.pos >> 16) & 0xFFFF)) << 16) >> 16);
}

fn get_uv(v: Vertex) -> vec2<i32> {
    let x = v.pos & 0xFFFF;
    let y = (v.pos >> 16) & 0xFFFF;
    return vec2(i32(x), i32(y));
}

fn get_flag(data: Vertex, idx: u32) -> bool {
    return (data.texpage_base_and_flags & (u32(1) << (idx + 16))) != 0;
}

fn get_textured(data: Vertex) -> bool {
    return get_flag(data, 0);
}

fn get_depth(v: Vertex) -> u32 {
    return (v.color >> 24) & 0xFF;
}

@group(0) @binding(0) var vram_t: texture_storage_2d<r32uint, read_write>;
@group(0) @binding(1) var<storage, read> vertex_buffer: array<Vertex>;
@group(0) @binding(2) var<storage, read> uniforms: Uniforms;
@group(0) @binding(3) var<storage, read_write> bins_buffer: array<u32>;
@group(0) @binding(4) var<storage, read_write> partial_derivatives: array<PartialDerivative>;

// fn draw_triangle(v1: Vertex, v2: Vertex, v3: Vertex, bounds: vec4<i32>) {
//     // let part = get_partition(id);
//     // let triangle_bounds = get_bounds(get_pos(v1), get_pos(v2), get_pos(v3));
//     // let bounds = intersect(part, triangle_bounds);
//
//     for (var y = bounds.y; y <= bounds.w; y++) {
//         for (var x = bounds.x; x <= bounds.z; x++) {
//             let lambda = barycentric(get_pos(v1), get_pos(v2), get_pos(v3), x, y);
//             if lambda.x < 0.0 || lambda.y < 0.0 || lambda.z < 0.0 {
//                 continue;
//             }
//
//             // let c = pack_color(vec3<f32>(1.0, 1.0, 1.0),false);
//             let col = interpolate_color(lambda, v1, v2, v3);
//             let c = pack_color(col, false);
//             // compose_buffer[1024 * y + x + (1024 * 512 * z)] = c | (1 << 16);
//             textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(c, 0, 0, 0));
//             // vram[1024*y + x] = v1.color;
//         }
//     }
// }

fn draw_triangle(v1: Vertex, v2: Vertex, v3: Vertex, bounds: vec4<i32>) {
    let p0 = get_pos(v1);
    let p1 = get_pos(v2);
    let p2 = get_pos(v3);
    let triangle_bounds = get_bounds(p0, p1, p2);
    let bds = intersect(bounds, triangle_bounds);

    let start = bds.xy;
    var edge1 = edge_function(start, p0, p1);// let (mut e1_row, a1, b1) 
    var edge2 = edge_function(start, p1, p2);
    var edge3 = edge_function(start, p2, p0);

    let bias1 = i32(!is_top_left(p0, p1)); // could be precomputed
    let bias2 = i32(!is_top_left(p1, p2));
    let bias3 = i32(!is_top_left(p2, p0));

    for (var y = bds.y; y <= bds.w; y++) {
        var e1 = edge1.x;
        var e2 = edge2.x;
        var e3 = edge3.x;
        for (var x = bds.x; x <= bds.z; x++) {
            if e1 >= bias1 && e2 >= bias2 && e3 >= bias3 {

                // let c = pack_color(vec3<f32>(1.0, 1.0, 1.0),false);
                // let col = interpolate_color(lambda, v1, v2, v3);
                let col = rgb8_split_color(v1.color);
                let c = pack_color(col, false);
                // compose_buffer[1024 * y + x + (1024 * 512 * z)] = c | (1 << 16);
                textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(c, 0, 0, 0));
                // vram[1024*y + x] = v1.color;
            }
            e1 += edge1.y;
            e2 += edge2.y;
            e3 += edge3.y;
        }
        edge1.x += edge1.z;
        edge2.x += edge2.z;
        edge3.x += edge3.z;
    }
}

fn draw_triangle_shaded(idx: u32, v1: Vertex, v2: Vertex, v3: Vertex, bounds: vec4<i32>) {
    let p0 = get_pos(v1);
    let p1 = get_pos(v2);
    let p2 = get_pos(v3);

    let c0 = rgb8_split_color(v1.color) * 255;
    let c1 = rgb8_split_color(v2.color) * 255;
    let c2 = rgb8_split_color(v3.color) * 255;

    let triangle_bounds = get_bounds(p0, p1, p2);
    let bds = intersect(bounds, triangle_bounds);

    let start = bds.xy;
    var edge1 = edge_function(start, p0, p1);// let (mut e1_row, a1, b1)
    var edge2 = edge_function(start, p1, p2);
    var edge3 = edge_function(start, p2, p0);

    let bias1 = i32(!is_top_left(p0, p1)); // could be precomputed
    let bias2 = i32(!is_top_left(p1, p2));
    let bias3 = i32(!is_top_left(p2, p0));

    let d = partial_derivatives[idx];
    let r_dx = d.r_dx;
    let g_dx = d.g_dx;
    let b_dx = d.b_dx;

    let r_dy = d.r_dy;
    let g_dy = d.g_dy;
    let b_dy = d.b_dy;

    // let r_dx = edge2.y * i32(c0.r) + edge3.y * i32(c1.r) + edge1.y * i32(c2.r);
    // let g_dx = edge2.y * i32(c0.g) + edge3.y * i32(c1.g) + edge1.y * i32(c2.g);
    // let b_dx = edge2.y * i32(c0.b) + edge3.y * i32(c1.b) + edge1.y * i32(c2.b);
    //
    // let r_dy = edge2.z * i32(c0.r) + edge3.z * i32(c1.r) + edge1.z * i32(c2.r);
    // let g_dy = edge2.z * i32(c0.g) + edge3.z * i32(c1.g) + edge1.z * i32(c2.g);
    // let b_dy = edge2.z * i32(c0.b) + edge3.z * i32(c1.b) + edge1.z * i32(c2.b);
    //
    var r_row = edge2.x * i32(c0.r) + edge3.x * i32(c1.r) + edge1.x * i32(c2.r);
    var g_row = edge2.x * i32(c0.g) + edge3.x * i32(c1.g) + edge1.x * i32(c2.g);
    var b_row = edge2.x * i32(c0.b) + edge3.x * i32(c1.b) + edge1.x * i32(c2.b);

    let sum = edge1.x + edge2.x + edge3.x;

    for (var y = bds.y; y <= bds.w; y++) {
        var e1 = edge1.x;
        var e2 = edge2.x;
        var e3 = edge3.x;
        var r_num = r_row;
        var g_num = g_row;
        var b_num = b_row;

        for (var x = bds.x; x <= bds.z; x++) {
            if e1 >= bias1 && e2 >= bias2 && e3 >= bias3 {

                let col = vec3<f32>(
                    f32(r_num / sum),
                    f32(g_num / sum),
                    f32(b_num / sum),
                ) / 255.0;

                // let c = pack_color(vec3<f32>(1.0, 1.0, 1.0),false);
                // let col = interpolate_color(lambda, v1, v2, v3);
                // let col = rgb8_split_color(v1.color);
                let c = pack_color(col, false);
                // compose_buffer[1024 * y + x + (1024 * 512 * z)] = c | (1 << 16);
                textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(c, 0, 0, 0));
                // vram[1024*y + x] = v1.color;
            }
            e1 += edge1.y;
            e2 += edge2.y;
            e3 += edge3.y;
            r_num += r_dx;
            g_num += g_dx;
            b_num += b_dx;
        }
        edge1.x += edge1.z;
        edge2.x += edge2.z;
        edge3.x += edge3.z;
        r_row += r_dy;
        g_row += g_dy;
        b_row += b_dy;
    }
}

fn draw_triangle_shaded_textured(idx: u32, v1: Vertex, v2: Vertex, v3: Vertex, bounds: vec4<i32>) {
    let p0 = get_pos(v1);
    let p1 = get_pos(v2);
    let p2 = get_pos(v3);

    let c0 = rgb8_split_color(v1.color) * 255;
    let c1 = rgb8_split_color(v2.color) * 255;
    let c2 = rgb8_split_color(v3.color) * 255;

    let triangle_bounds = get_bounds(p0, p1, p2);
    let bds = intersect(bounds, triangle_bounds);

    let is_textured = get_textured(v1);

    let start = bds.xy;
    var edge1 = edge_function(start, p0, p1);// let (mut e1_row, a1, b1)
    var edge2 = edge_function(start, p1, p2);
    var edge3 = edge_function(start, p2, p0);

    let bias1 = i32(!is_top_left(p0, p1)); // could be precomputed
    let bias2 = i32(!is_top_left(p1, p2));
    let bias3 = i32(!is_top_left(p2, p0));

    let d = partial_derivatives[idx];
    let r_dx = d.r_dx;
    let g_dx = d.g_dx;
    let b_dx = d.b_dx;

    let r_dy = d.r_dy;
    let g_dy = d.g_dy;
    let b_dy = d.b_dy;

    // let r_dx = edge2.y * i32(c0.r) + edge3.y * i32(c1.r) + edge1.y * i32(c2.r);
    // let g_dx = edge2.y * i32(c0.g) + edge3.y * i32(c1.g) + edge1.y * i32(c2.g);
    // let b_dx = edge2.y * i32(c0.b) + edge3.y * i32(c1.b) + edge1.y * i32(c2.b);
    //
    // let r_dy = edge2.z * i32(c0.r) + edge3.z * i32(c1.r) + edge1.z * i32(c2.r);
    // let g_dy = edge2.z * i32(c0.g) + edge3.z * i32(c1.g) + edge1.z * i32(c2.g);
    // let b_dy = edge2.z * i32(c0.b) + edge3.z * i32(c1.b) + edge1.z * i32(c2.b);
    //
    var r_row = edge2.x * i32(c0.r) + edge3.x * i32(c1.r) + edge1.x * i32(c2.r);
    var g_row = edge2.x * i32(c0.g) + edge3.x * i32(c1.g) + edge1.x * i32(c2.g);
    var b_row = edge2.x * i32(c0.b) + edge3.x * i32(c1.b) + edge1.x * i32(c2.b);

    let uv0 = get_uv(v1);
    let uv1 = get_uv(v2);
    let uv2 = get_uv(v3);

    let u_dx = edge2.y * uv0.x + edge3.y * uv1.x + edge1.y * uv2.x;
    let v_dx = edge2.y * uv0.y + edge3.y * uv1.y + edge1.y * uv2.y;
    let u_dy = edge2.z * uv0.x + edge3.z * uv1.x + edge1.z * uv2.x;
    let v_dy = edge2.z * uv0.y + edge3.z * uv1.y + edge1.z * uv2.y;

    var u_row = edge2.x * uv0.x + edge3.x * uv1.x + edge1.x * uv2.x;
    var v_row = edge2.x * uv0.y + edge3.x * uv1.y + edge1.x * uv2.y;

    let sum = edge1.x + edge2.x + edge3.x;

    for (var y = bds.y; y <= bds.w; y++) {
        var e1 = edge1.x;
        var e2 = edge2.x;
        var e3 = edge3.x;
        var r_num = r_row;
        var g_num = g_row;
        var b_num = b_row;
        var u_num = u_row;
        var v_num = v_row;

        for (var x = bds.x; x <= bds.z; x++) {
            if e1 >= bias1 && e2 >= bias2 && e3 >= bias3 {

                if is_textured {
                    let tex = get_texel(v1, vec2(u_num / sum, v_num / sum));
                    if tex != 0 {
                        textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(tex, 0, 0, 0));
                    }
                } else {

                    let col = vec3<f32>(
                        f32(r_num / sum),
                        f32(g_num / sum),
                        f32(b_num / sum),
                    ) / 255.0;

                    // let c = pack_color(vec3<f32>(1.0, 1.0, 1.0),false);
                    // let col = interpolate_color(lambda, v1, v2, v3);
                    // let col = rgb8_split_color(v1.color);
                    let c = pack_color(col, false);
                    textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(c, 0, 0, 0));
                }
                // vram[1024*y + x] = v1.color;
            }
            e1 += edge1.y;
            e2 += edge2.y;
            e3 += edge3.y;
            r_num += r_dx;
            g_num += g_dx;
            b_num += b_dx;
            u_num += u_dx;
            v_num += v_dx;
        }
        edge1.x += edge1.z;
        edge2.x += edge2.z;
        edge3.x += edge3.z;
        r_row += r_dy;
        g_row += g_dy;
        b_row += b_dy;
        u_row += u_dy;
        v_row += v_dy;
    }
}

fn get_texel(v: Vertex, uv: vec2<i32>) -> u32 {
    let depth = get_depth(v);
    switch depth {
        case 0: {
        }
        case 1: {
        }
        default: {
        }
    };
    // 1. compute_texel_offset...
    return 0;
}

fn edge_function(p: vec2<i32>, p0: vec2<i32>, p1: vec2<i32>) -> vec3<i32> {
    let a = p0.y - p1.y;
    let b = p1.x - p0.x;
    let e = b * (p.y - p0.y) + a * (p.x - p0.x);
    return vec3(e, a, b);
}

// Test that vertices v0, v1, v2 are in clockwise order
// fn needs_vertex_reordering(t: &[Vec2; 3]) -> bool {
//     ((t[2].x - t[0].x) * (t[1].y - t[0].y) - (t[1].x - t[0].x) * (t[2].y - t[0].y)) > 0
// }
    // Test if edge AB is a top or left edge
fn is_top_left(a: vec2<i32>, b: vec2<i32>) -> bool {
    if a.y == b.y {
        return a.x < b.x;
    } else {
        return a.y > b.y;
    }
}

fn interpolate_color(lambda: vec3<f32>, v0: Vertex, v1: Vertex, v2: Vertex) -> vec3<f32> {
    let c0 = rgb8_split_color(v0.color);
    let c1 = rgb8_split_color(v1.color);
    let c2 = rgb8_split_color(v2.color);
    let r = min(1.0, (lambda[0] * c0[0] + lambda[1] * c1[0] + lambda[2] * c2[0]));
    let g = min(1.0, (lambda[0] * c0[1] + lambda[1] * c1[1] + lambda[2] * c2[1]));
    let b = min(1.0, (lambda[0] * c0[2] + lambda[1] * c1[2] + lambda[2] * c2[2]));
    return vec3<f32>(r, g, b);
}

fn rgb8_split_color(value: u32) -> vec3<f32> {
    // unpack4x8unorm unpacks as
    // 0xrrggbb
    // we need
    // 0xbbggrr (r is lsb)
    return unpack4x8unorm(value & 0x00FFFFFF).rgb;
}

fn pack_color(color: vec3<f32>, mask: bool) -> u32 {
    let r = u32(color.r * 31.0);
    let g = u32(color.g * 31.0);
    let b = u32(color.b * 31.0);
    var c = r | (g << 5u) | (b << 10u);
    if mask {
        c |= (1 << 15u);
    }
    return c;
}

fn barycentric(v1: vec2<i32>, v2: vec2<i32>, v3: vec2<i32>, x: i32, y: i32) -> vec3<f32> {
    let u = cross(
        vec3<f32>(f32(v3.x) - f32(v1.x), f32(v2.x) - f32(v1.x), f32(v1.x - x)),
        vec3<f32>(f32(v3.y) - f32(v1.y), f32(v2.y) - f32(v1.y), f32(v1.y) - f32(y))
    );

    if abs(u.z) < 1.0 {
        return vec3<f32>(-1.0, 1.0, 1.0);
    }

    return vec3<f32>(1.0 - (u.x + u.y) / u.z, u.y / u.z, u.x / u.z);
}

fn get_partition(id: vec2<u32>) -> vec4<i32> {
    var width_bin_x = (uniforms.drawing_area_right - uniforms.drawing_area_left) / 128;
    if width_bin_x * 128 < (uniforms.drawing_area_right - uniforms.drawing_area_left) {
        width_bin_x++;
    }
    var width_bin_y = (uniforms.drawing_area_bottom - uniforms.drawing_area_top) / 64;
    if width_bin_y * 64 < (uniforms.drawing_area_bottom - uniforms.drawing_area_top) {
        width_bin_y++;
    }
    // let width_x = 1024u / 8;
    // let width_y = 512u / 8;
    return vec4<i32>(
        i32(id.x) * width_bin_x + uniforms.drawing_area_left,
        i32(id.y) * width_bin_y + uniforms.drawing_area_top,
        (i32(id.x) + 1) * width_bin_x - 1 + uniforms.drawing_area_left,
        (i32(id.y) + 1) * width_bin_y - 1 + uniforms.drawing_area_top,
    );
}
fn get_bounds(v1: vec2<i32>, v2: vec2<i32>, v3: vec2<i32>) -> vec4<i32> {
    var min_max = vec4<i32>();
    min_max.x = min(min(v1.x, v2.x), v3.x);
    min_max.y = min(min(v1.y, v2.y), v3.y);
    min_max.z = max(max(v1.x, v2.x), v3.x);
    min_max.w = max(max(v1.y, v2.y), v3.y);

    return min_max;
}

fn intersect(a: vec4<i32>, b: vec4<i32>) -> vec4<i32> {
    let cx = max(a.x, b.x);
    let cy = max(a.y, b.y);
    let dx = min(a.z, b.z);
    let dy = min(a.w, b.w);
    if cx >= dx || cy >= dy {
        return vec4<i32>(0);
    }
    return vec4<i32>(cx, cy, dx, dy);
}

@compute @workgroup_size(8,8,1)
fn main(
    @builtin(local_invocation_id) local_invocation_id: vec3<u32>, // actual ids of threads...
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local_invocation_index: u32,
    @builtin(num_workgroups) num_workgroups: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
) {
    // let workgroup_index = workgroup_id.x +
    //  workgroup_id.y * num_workgroups.x +
    //  workgroup_id.z * num_workgroups.x * num_workgroups.y;
    //
    // let global_invocation_index = workgroup_index * 64 +
    //  local_invocation_index;

    // let idx = global_invocation_index * 3;
    // let idx = global_id.x * 3u;
    // let idx = workgroup_id.x * 3u;

    // if idx >= arrayLength(&vertex_buffer) {
    //     return;
    // }
    let bin = vec2(workgroup_id.x * 8, workgroup_id.y * 8) + local_invocation_id.xy;
    let bounds = get_partition(bin);
    for (var i: u32 = 0; i < 64; i++) {
        let idx = (64 * (bin.y * 128 + bin.x)) + i;
        let v_idx = bins_buffer[idx];
        if v_idx == 0 {
            continue;
        }
        bins_buffer[idx] = 0;
        let vert_idx = v_idx - 1;
        var v1 = vertex_buffer[vert_idx + 0u];
        var v2 = vertex_buffer[vert_idx + 1u];
        var v3 = vertex_buffer[vert_idx + 2u];
        draw_triangle_shaded(vert_idx, v1, v2, v3, bounds);
        // can clean up here in principle...
    }

    // now we have local_invocation_id.xy to play with...

    // v2.pos = (36 +  (36 << 16));
    // v1.pos = ((150 +  (86 << 16)));
    // v3.pos = (72 + (136 << 16));

    // draw_triangle(v1, v2, v3, local_invocation_id.xy, workgroup_id.x);
    // draw_anything(local_invocation_id.xy);

    // workgroupBarrier();
}

// var<workgroup> bin_indices: array<atomic<u32>,8192>; // 64*128
@compute @workgroup_size(8,8,1)
fn bin(
    @builtin(local_invocation_id) local_invocation_id: vec3<u32>, // actual ids of threads...
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local_invocation_index: u32,
    @builtin(num_workgroups) num_workgroups: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
) {
    // zero out indices...
    // atomicStore(&bin_indices[local_invocation_index], 0);
    // clear bins

    // let offs = local_invocation_index*64*128;
    // let end = offs + 64 * 128;
    // for (var i: u32 = offs; i < end; i++) {
    //     bins_buffer[i] = 0xFFFF;
    // }
    // workgroupBarrier();
    // let workgroup_index = workgroup_id.x +
    //  workgroup_id.y * num_workgroups.x +
    //  workgroup_id.z * num_workgroups.x * num_workgroups.y;
    //
    // let global_invocation_index = workgroup_index * 64 +
    //  local_invocation_index;

    // let idx = global_invocation_index * 3;
    // let idx = global_id.x * 3u;
    // let idx = workgroup_id.x * 3u;

    // if idx >= arrayLength(&vertex_buffer) {
    //     return;
    // }
    // let bounds = get_partition(local_invocation_id.xy);
    // for (var i: u32 = 0; i < 64; i++) {
    //     let idx = ((3 * 64) * (local_invocation_id.y * 8 + local_invocation_id.x)) + 3*i;
    //     var v1 = vertex_buffer[idx + 0u];
    //     var v2 = vertex_buffer[idx + 1u];
    //     var v3 = vertex_buffer[idx + 2u];
    //     if v1.texpage_base_and_flags != 0 {
    //         draw_triangle(v1, v2, v3, bounds);
    //     }
    // }
    let idx = local_invocation_index * 3;
    if idx >= uniforms.num_vertices {
        return;
    }
    var v1 = vertex_buffer[idx + 0u];
    var v2 = vertex_buffer[idx + 1u];
    var v3 = vertex_buffer[idx + 2u];

    let p0 = get_pos(v1);
    let p1 = get_pos(v2);
    let p2 = get_pos(v3);

    let min_x = max(uniforms.drawing_area_left, min(p0.x, min(p1.x, p2.x)));
    let min_y = max(uniforms.drawing_area_top, min(p0.y, min(p1.y, p2.y)));
    let max_x = min(uniforms.drawing_area_right, max(p0.x, max(p1.x, p2.x)));
    let max_y = min(uniforms.drawing_area_bottom, max(p0.y, max(p1.y, p2.y)));

    // precompute_partial_derivatives(idx, v1, v2, v3);

    let c0 = rgb8_split_color(v1.color) * 255;
    let c1 = rgb8_split_color(v2.color) * 255;
    let c2 = rgb8_split_color(v3.color) * 255;

    let triangle_bounds = get_bounds(p0, p1, p2);

    let start = triangle_bounds.xy;
    var edge1 = edge_function(start, p0, p1);// let (mut e1_row, a1, b1)
    var edge2 = edge_function(start, p1, p2);
    var edge3 = edge_function(start, p2, p0);

    let bias1 = i32(!is_top_left(p0, p1)); // could be precomputed
    let bias2 = i32(!is_top_left(p1, p2));
    let bias3 = i32(!is_top_left(p2, p0));

    let r_dx = edge2.y * i32(c0.r) + edge3.y * i32(c1.r) + edge1.y * i32(c2.r);
    let g_dx = edge2.y * i32(c0.g) + edge3.y * i32(c1.g) + edge1.y * i32(c2.g);
    let b_dx = edge2.y * i32(c0.b) + edge3.y * i32(c1.b) + edge1.y * i32(c2.b);

    let r_dy = edge2.z * i32(c0.r) + edge3.z * i32(c1.r) + edge1.z * i32(c2.r);
    let g_dy = edge2.z * i32(c0.g) + edge3.z * i32(c1.g) + edge1.z * i32(c2.g);
    let b_dy = edge2.z * i32(c0.b) + edge3.z * i32(c1.b) + edge1.z * i32(c2.b);
    partial_derivatives[idx] = PartialDerivative(r_dx, g_dx, b_dx, r_dy, g_dy, b_dy);
    // ----------
    var width_bin_x = (uniforms.drawing_area_right - uniforms.drawing_area_left) / 128;
    if width_bin_x * 128 < (uniforms.drawing_area_right - uniforms.drawing_area_left) {
        width_bin_x++;
    }
    var width_bin_y = (uniforms.drawing_area_bottom - uniforms.drawing_area_top) / 64;
    if width_bin_y * 64 < (uniforms.drawing_area_bottom - uniforms.drawing_area_top) {
        width_bin_y++;
    }

    // helps, but only little...
    let start_bin_x = max((min_x - uniforms.drawing_area_left) / width_bin_x, i32(workgroup_id.x) * 32);
    let end_bin_x = min((max_x - uniforms.drawing_area_left) / width_bin_x, (i32(workgroup_id.x) + 1) * 32);
    let start_bin_y = max((min_y - uniforms.drawing_area_top) / width_bin_y, i32(workgroup_id.y) * 16);
    let end_bin_y = min((max_y - uniforms.drawing_area_top) / width_bin_y, i32(workgroup_id.y + 1) * 16);

    for (var y = start_bin_y; y <= end_bin_y; y++) {
        for (var x = start_bin_x; x <= end_bin_x; x++) {
            // let tl = 64 * y + 8*x;
            // let tr = 64 * y + 8*(x+1) -1;
            // let bl = 64 * (y+1) + 8*x - 8;
            // let br = 64 * (y+1) + 8*(x+1) - 1 - 8;
            // if edge_inside_triangle(start, 8*x, 8*y, edge1, edge2, edge3, bias1, bias2, bias3) ||
            //    edge_inside_triangle(start, 8*x, 8*(y+1)-1, edge1, edge2, edge3, bias1, bias2, bias3) ||
            //    edge_inside_triangle(start, 8*(x+1)-1, 8*y, edge1, edge2, edge3, bias1, bias2, bias3) ||
            //    edge_inside_triangle(start, 8*(x+1) - 1, 8*(y+1) - 1, edge1, edge2, edge3, bias1, bias2, bias3) {
            bins_buffer[64 * (u32(y) * 128 + u32(x)) + local_invocation_index] = idx + 1;
            // }
        }
    }

    //     for y in start_bin_y..=end_bin_y {
    //         for x in start_bin_x..=end_bin_x {
    //             vertices[3*64*(y*8 + x) + 3*i] = vs[0];
    //             vertices[3*64*(y*8 + x) + 3*i + 1] = vs[1];
    //             vertices[3*64*(y*8 + x) + 3*i + 2] = vs[2];
    //         }
    //     }

    // now we have local_invocation_id.xy to play with...

    // v2.pos = (36 +  (36 << 16));
    // v1.pos = ((150 +  (86 << 16)));
    // v3.pos = (72 + (136 << 16));

    // draw_triangle(v1, v2, v3, local_invocation_id.xy, workgroup_id.x);
    // draw_anything(local_invocation_id.xy);

    // workgroupBarrier();
}

fn edge_inside_triangle(p0: vec2<i32>, x: i32, y: i32, edge1: vec3<i32>, edge2: vec3<i32>, edge3: vec3<i32>, bias1: i32, bias2: i32, bias3: i32) -> bool {
    let e1 = edge1.x + edge1.z * (y - p0.y) + edge1.y * (x - p0.x);
    let e2 = edge2.x + edge2.z * (y - p0.y) + edge2.y * (x - p0.x);
    let e3 = edge3.x + edge3.z * (y - p0.y) + edge3.y * (x - p0.x);
    return e1 >= 0 && e2 >= 0 && e3 >= 0;
    // edge1.x = edge1.z * (y-p0.y) + edge1.y * (x-p0.x);
    // edge2.x = edge2.z * (y-p0.y) + edge2.y * (x-p0.x);
    // edge3.x = edge3.z * (y-p0.y) + edge3.y * (x-p0.x);
}

// fn precompute_partial_derivatives(idx: u32, v1: Vertex, v2: Vertex, v3: Vertex) {
// }

// size: ((64) * 64 * 128 ),
// vertices * ygroups * xgroups...
/// too slow
@compute @workgroup_size(8,8,1)
fn clear_bins(
    @builtin(local_invocation_id) local_invocation_id: vec3<u32>, // actual ids of threads...
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local_invocation_index: u32,
    @builtin(num_workgroups) num_workgroups: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
) {
    //                    128                                      64
    let idx = (64 * (workgroup_id.y * 16 + workgroup_id.x)) + local_invocation_index;
    let start = 64 * idx;
    let end = 64 * (idx + 1);
    for (var i = start; i < end; i++) {
        bins_buffer[i] = 0xFFFF;
    }
}
                // bins_buffer[64*(y*128 + x) + local_invocation_index] = idx;
