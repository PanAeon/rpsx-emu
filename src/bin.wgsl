
struct Uniforms {
    drawing_area_left: i32,
    drawing_area_top: i32,
    drawing_area_bottom: i32,
    drawing_area_right: i32,
    num_vertices: u32,
    fill_color: u32,
};

struct Vertex {
    pos: u32,
    color: u32,
    uv: u32,
    clut: u32,
    texture_window: u32,
    texpage_base_and_flags: u32,
};

fn get_pos(v: Vertex) -> vec2<i32> {
    let _x = v.pos & 0xFFFF;
    let _y = (v.pos >> 16) & 0xFFFF;
    let x = (_x & 0x7FFF) - (_x & 0x8000);
    let y = (_y & 0x7FFF) - (_y & 0x8000);
    return vec2(i32(x), i32(y));
    // return vec2<i32>((i32(v.pos & 0xFFFF) << 16) >> 16, ((i32((v.pos >> 16) & 0xFFFF)) << 16) >> 16);
}

fn get_bounds(v1: vec2<i32>, v2: vec2<i32>, v3: vec2<i32>) -> vec4<i32> {
    var min_max = vec4<i32>();
    min_max.x = min(min(v1.x, v2.x), v3.x);
    min_max.y = min(min(v1.y, v2.y), v3.y);
    min_max.z = max(max(v1.x, v2.x), v3.x);
    min_max.w = max(max(v1.y, v2.y), v3.y);

    return min_max;
}

@group(0) @binding(0) var<storage, read> vertex_buffer: array<Vertex>;
@group(0) @binding(1) var<storage, read> uniforms: Uniforms;
// @group(0) @binding(2) var<storage, read_write> bin_indices: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read_write> bins_buffer: array<u32>;

// var<workgroup> bin_indices: array<atomic<u32>,8192>; // 64*128
// ok, doesn't work...., thinking... thinking moar....
@compute @workgroup_size(8,8,1)
fn bin(
    @builtin(local_invocation_id) local_id: vec3<u32>, // actual ids of threads...
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local_invocation_index: u32,
    @builtin(num_workgroups) num_workgroups: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
) {
    // zero out indices...
    // for (var i = 0u; i < 128; i++) {
    //     atomicStore(&bin_indices[128 * local_invocation_index + i], 0);
    // }
    // workgroupBarrier();

    var width_bin_x = (uniforms.drawing_area_right - uniforms.drawing_area_left) / 128;
    if width_bin_x * 128 < (uniforms.drawing_area_right - uniforms.drawing_area_left) {
        width_bin_x++;
    }
    var width_bin_y = (uniforms.drawing_area_bottom - uniforms.drawing_area_top) / 64;
    if width_bin_y * 64 < (uniforms.drawing_area_bottom - uniforms.drawing_area_top) {
        width_bin_y++;
    }
    let bin = vec2(workgroup_id.x * 8, workgroup_id.y * 8) + local_id.xy;
    let bin_idx = (64 * (bin.y * 128 + bin.x));

    var start = 0u;

    /*for (var i = 0u; i < uniforms.num_vertices; i++) {
        let idx = i * 3;
        var v1 = vertex_buffer[idx + 0u];
        var v2 = vertex_buffer[idx + 1u];
        var v3 = vertex_buffer[idx + 2u];

        let p0 = get_pos(v1);
        let p1 = get_pos(v2);
        let p2 = get_pos(v3);

        // let min_x = max(uniforms.drawing_area_left, min(p0.x, min(p1.x, p2.x)));
        // let min_y = max(uniforms.drawing_area_top, min(p0.y, min(p1.y, p2.y)));
        // let max_x = min(uniforms.drawing_area_right, max(p0.x, max(p1.x, p2.x)));
        // let max_y = min(uniforms.drawing_area_bottom, max(p0.y, max(p1.y, p2.y)));

        let triangle_bounds = get_bounds(p0, p1, p2);

        if triangle_bounds.z >= (i32(bin.x) * width_bin_x) &&
           triangle_bounds.x < (i32(bin.x + 1) * width_bin_x) &&
           triangle_bounds.w >= (i32(bin.y) * width_bin_y) &&
           triangle_bounds.y < (i32(bin.y + 1) * width_bin_y) {

            bins_buffer[bin_idx + start] = idx + 1;
            start += 1;
        }

        // let tl = 64 * y + 8*x;
        // let tr = 64 * y + 8*(x+1) -1;
        // let bl = 64 * (y+1) + 8*x - 8;
        // let br = 64 * (y+1) + 8*(x+1) - 1 - 8;
        // if edge_inside_triangle(start, 8*x, 8*y, edge1, edge2, edge3, bias1, bias2, bias3) ||
        //    edge_inside_triangle(start, 8*x, 8*(y+1)-1, edge1, edge2, edge3, bias1, bias2, bias3) ||
        //    edge_inside_triangle(start, 8*(x+1)-1, 8*y, edge1, edge2, edge3, bias1, bias2, bias3) ||
        //    edge_inside_triangle(start, 8*(x+1) - 1, 8*(y+1) - 1, edge1, edge2, edge3, bias1, bias2, bias3) {
        // let bin_idx = atomicAdd(&bin_indices[u32(y) * 128 + u32(x)], 1);
        // bins_buffer[64 * (u32(y) * 128 + u32(x)) + bin_idx] = idx + 1;
        // bins_buffer[64 * (u32(y) * 128 + u32(x)) + local_invocation_index] = idx + 1;
        // }
    }*/

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

