
// for now lazy path - just split whole 1024x512 drawing area, than think of something...
// but! we need to implement scissors anyway...
struct Uniforms {
    drawing_area_offset_x: u32,
    drawing_area_offset_y: u32,
    drawing_area_width: u32,
    drawing_area_height: u32,
};

struct Vertex {
    pos: u32,
    color: u32,
    uv: u32,
    clut: u32,
    texture_window: u32,
    texpage_base_and_flags: u32,
};

fn get_pos(v: Vertex) -> vec2<u32> {
    return vec2<u32>(v.pos & 0xFFFF, (v.pos >> 16) & 0xFFFF);
}

@group(0) @binding(0) var vram_t: texture_storage_2d<r32uint, read_write>;
@group(0) @binding(1) var<storage, read> vertex_buffer: array<Vertex>;
@group(0) @binding(2) var<storage, read> uniforms: Uniforms;
@group(0) @binding(3) var<storage, read_write> bins_buffer: array<u32>;

fn draw_triangle(v1: Vertex, v2: Vertex, v3: Vertex, bounds: vec4<u32>) {
    // let part = get_partition(id);
    // let triangle_bounds = get_bounds(get_pos(v1), get_pos(v2), get_pos(v3));
    // let bounds = intersect(part, triangle_bounds);

    for (var y = bounds.y; y <= bounds.w; y++) {
        for (var x = bounds.x; x <= bounds.z; x++) {
            let lambda = barycentric(get_pos(v1), get_pos(v2), get_pos(v3), x, y);
            if lambda.x < 0.0 || lambda.y < 0.0 || lambda.z < 0.0 {
                continue;
            }

            // let c = pack_color(vec3<f32>(1.0, 1.0, 1.0),false);
            let col = interpolate_color(lambda, v1, v2, v3);
            let c = pack_color(col, false);
            // compose_buffer[1024 * y + x + (1024 * 512 * z)] = c | (1 << 16);
            textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(c, 0, 0, 0));
            // vram[1024*y + x] = v1.color;
        }
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

fn barycentric(v1: vec2<u32>, v2: vec2<u32>, v3: vec2<u32>, x: u32, y: u32) -> vec3<f32> {
    let u = cross(
        vec3<f32>(f32(v3.x) - f32(v1.x), f32(v2.x) - f32(v1.x), f32(v1.x - x)),
        vec3<f32>(f32(v3.y) - f32(v1.y), f32(v2.y) - f32(v1.y), f32(v1.y) - f32(y))
    );

    if abs(u.z) < 1.0 {
        return vec3<f32>(-1.0, 1.0, 1.0);
    }

    return vec3<f32>(1.0 - (u.x + u.y) / u.z, u.y / u.z, u.x / u.z);
}

fn get_partition(id: vec2<u32>) -> vec4<u32> {
    // let width_x = 1024u / 8;
    // let width_y = 512u / 8;
    return vec4<u32>(
        (id.x) * 128,
        (id.y) * 64,
        ((id.x) + 1) * 128 - 1,
        ((id.y) + 1) * 64 - 1,
    );
}
fn get_bounds(v1: vec2<u32>, v2: vec2<u32>, v3: vec2<u32>) -> vec4<u32> {
    var min_max = vec4<u32>();
    min_max.x = min(min(v1.x, v2.x), v3.x);
    min_max.y = min(min(v1.y, v2.y), v3.y);
    min_max.z = max(max(v1.x, v2.x), v3.x);
    min_max.w = max(max(v1.y, v2.y), v3.y);

    return min_max;
}

fn intersect(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    let cx = max(a.x, b.x);
    let cy = max(a.y, b.y);
    let dx = min(a.z, b.z);
    let dy = min(a.w, b.w);
    if cx >= dx || cy >= dy {
        return vec4<u32>(0);
    }
    return vec4<u32>(cx, cy, dx, dy);
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
    let bounds = get_partition(local_invocation_id.xy);
    for (var i: u32 = 0; i < 64; i++) {
        let idx = (64 * (local_invocation_id.y * 8 + local_invocation_id.x)) + i;
        let vert_idx = bins_buffer[idx];
        if vert_idx == 0xFFFF {
            return;
        }
        var v1 = vertex_buffer[vert_idx + 0u];
        var v2 = vertex_buffer[vert_idx + 1u];
        var v3 = vertex_buffer[vert_idx + 2u];
        draw_triangle(v1, v2, v3, bounds);
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
//
// var<workgroup> bin_indices: array<atomic<u32>,64>;
// @compute @workgroup_size(8,8,1)
// fn bin(
//     @builtin(local_invocation_id) local_invocation_id: vec3<u32>, // actual ids of threads...
//     @builtin(global_invocation_id) global_id: vec3<u32>,
//     @builtin(local_invocation_index) local_invocation_index: u32,
//     @builtin(num_workgroups) num_workgroups: vec3<u32>,
//     @builtin(workgroup_id) workgroup_id: vec3<u32>,
// ) {
//     // zero out indices...
//     atomicStore(&bin_indices[local_invocation_index], 0);
//     // bin_indices[local_invocation_index] = 0;
//     workgroupBarrier();
//     // let workgroup_index = workgroup_id.x +
//     //  workgroup_id.y * num_workgroups.x +
//     //  workgroup_id.z * num_workgroups.x * num_workgroups.y;
//     //
//     // let global_invocation_index = workgroup_index * 64 +
//     //  local_invocation_index;
//
//     // let idx = global_invocation_index * 3;
//     // let idx = global_id.x * 3u;
//     // let idx = workgroup_id.x * 3u;
//
//     // if idx >= arrayLength(&vertex_buffer) {
//     //     return;
//     // }
//     // let bounds = get_partition(local_invocation_id.xy);
//     // for (var i: u32 = 0; i < 64; i++) {
//     //     let idx = ((3 * 64) * (local_invocation_id.y * 8 + local_invocation_id.x)) + 3*i;
//     //     var v1 = vertex_buffer[idx + 0u];
//     //     var v2 = vertex_buffer[idx + 1u];
//     //     var v3 = vertex_buffer[idx + 2u];
//     //     if v1.texpage_base_and_flags != 0 {
//     //         draw_triangle(v1, v2, v3, bounds);
//     //     }
//     // }
//     let idx = local_invocation_index * 3;
//     var v1 = vertex_buffer[idx + 0u];
//     var v2 = vertex_buffer[idx + 1u];
//     var v3 = vertex_buffer[idx + 2u];
//     v1.texpage_base_and_flags = 1;
//     v2.texpage_base_and_flags = 1;
//     v3.texpage_base_and_flags = 1;
//
//     let p1 = get_pos(v1);
//     let p2 = get_pos(v2);
//     let p3 = get_pos(v3);
//
//     let min_x = min(p1.x, min(p2.x, p3.x));
//     let min_y = min(p1.y, min(p2.y, p3.y));
//     let max_x = max(p1.x, max(p2.x, p3.x));
//     let max_y = max(p1.y, max(p2.y, p3.y));
//
//     let start_bin_x = min_x / 128;
//     let end_bin_x = max_x / 128;
//     let start_bin_y = min_y / 64;
//     let end_bin_y = max_y / 64;
//
//     // let write_idx = 0;
//
//     for (var y = start_bin_y; y <= end_bin_y; y++) {
//         for (var x = start_bin_x; x <= end_bin_x; x++) {
//                 let write_idx = atomicAdd(&bin_indices[y*8+x], 1);
//                 bins_buffer[3*64*(y*8 + x) + write_idx] = idx;
//                 // bins_buffer[3*64*(y*8 + x) + 3*write_idx + 1] = v2;
//                 // bins_buffer[3*64*(y*8 + x) + 3*write_idx + 2] = v3;
//         }
//     }
//
//     //     for y in start_bin_y..=end_bin_y {
//     //         for x in start_bin_x..=end_bin_x {
//     //             vertices[3*64*(y*8 + x) + 3*i] = vs[0];
//     //             vertices[3*64*(y*8 + x) + 3*i + 1] = vs[1];
//     //             vertices[3*64*(y*8 + x) + 3*i + 2] = vs[2];
//     //         }
//     //     }
//
//     // now we have local_invocation_id.xy to play with...
//
//     // v2.pos = (36 +  (36 << 16));
//     // v1.pos = ((150 +  (86 << 16)));
//     // v3.pos = (72 + (136 << 16));
//
//     // draw_triangle(v1, v2, v3, local_invocation_id.xy, workgroup_id.x);
//     // draw_anything(local_invocation_id.xy);
//
//     // workgroupBarrier();
// }
//
// @compute @workgroup_size(8,8,1)
// fn clear_bins(
//     @builtin(local_invocation_id) local_invocation_id: vec3<u32>, // actual ids of threads...
//     @builtin(global_invocation_id) global_id: vec3<u32>,
//     @builtin(local_invocation_index) local_invocation_index: u32,
//     @builtin(num_workgroups) num_workgroups: vec3<u32>,
//     @builtin(workgroup_id) workgroup_id: vec3<u32>,
// ) {
//         let i = workgroup_id.x;
//         let idx = ((3 * 64) * (local_invocation_id.y * 8 + local_invocation_id.x)) + 3 * i;
//         bins_buffer[idx + 0u] = 0x3FF;
//     // for (var i: u32 = 0; i < 64; i++) {
//     //     let idx = ((3 * 64) * (local_invocation_id.y * 8 + local_invocation_id.x)) + 3 * i;
//     //     var v1 = bins_buffer[idx + 0u];
//     //     var v2 = bins_buffer[idx + 1u];
//     //     var v3 = bins_buffer[idx + 2u];
//     //     v1.texpage_base_and_flags = 0;
//     //     v2.texpage_base_and_flags = 0;
//     //     v3.texpage_base_and_flags = 0;
//     // }
// }
