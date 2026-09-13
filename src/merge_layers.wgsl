

@group(0) @binding(0) var vram_t: texture_storage_2d<r32uint, read_write>;
@group(0) @binding(1) var<storage, read_write> compose_buffer: array<u32>;

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

fn get_partition(id: vec2<u32>, workgroup_id: vec2<u32>) -> vec2<u32> {
    // let width_x = 1024u / 16;
    // let width_y = 512u / 8;
    let i_x = id.x + 16 * workgroup_id.x;
    let i_y = id.y + 8 * workgroup_id.y;
    return vec2<u32>(
        i_x,
        i_y,
    );
}

fn merge_layers(p: vec2<u32>) {
            for (var i: u32 = 0; i < 64u; i++) {
                let pixel = compose_buffer[1024 * p.y + p.x + (1024 * 512 * i)];
                if ((pixel >> 16u) & 0x1u) == 1u {
                    // compose it down...
                    textureStore(vram_t, p, vec4(pixel, 0, 0, 0));
                }
    }
}

fn clear_layers(p: vec2<u32>) {
            for (var i: u32 = 0; i < 64u; i++) {
                compose_buffer[1024 * p.y + p.x + (1024 * 512 * i)] = 0;
            }
}
// fn get_partition(id: vec2<u32>, workgroup_id: vec2<u32>) -> vec4<u32> {
//     let width_x = 1024u / 16;
//     let width_y = 512u / 8;
//     let i_x = id.x + 8 * workgroup_id.x;
//     let i_y = id.y + 8 * workgroup_id.y;
//     return vec4<u32>(
//         i_x * 8,
//         i_y * 8,
//         (i_x + 1) * 8 - 1,
//         (i_y + 1) * 8 - 1,
//     );
// }
//
// fn merge_layers(p: vec4<u32>) {
//     for (var x: u32 = p.x; x <= p.z; x++) {
//         for (var y: u32 = p.y; y <= p.w; y++) {
//             for (var i: u32 = 0; i < 64u; i++) {
//                 let pixel = compose_buffer[1024 * y + x + (1024 * 512 * i)];
//                 if ((pixel >> 16u) & 0x1u) == 1u {
//                     // compose it down...
//                     textureStore(vram_t, vec2(x, y), vec4(pixel, 0, 0, 0));
//                 }
//             }
//         }
//     }
// }
//
// fn clear_layers(p: vec4<u32>) {
//     for (var x: u32 = p.x; x <= p.z; x++) {
//         for (var y: u32 = p.y; y <= p.w; y++) {
//             for (var i: u32 = 0; i < 64u; i++) {
//                 compose_buffer[1024 * y + x + (1024 * 512 * i)] = 0;
//             }
//         }
//     }
// }

// 64 workgroups... (8x8)
@compute @workgroup_size(16,8,1)
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

    let part = get_partition(local_invocation_id.xy, workgroup_id.xy);

    merge_layers(part);

    // if idx >= arrayLength(&vertex_buffer) {
    //     return;
    // }

    // now we have local_invocation_id.xy to play with...
    // var v1 = vertex_buffer[idx + 0u];
    // var v2 = vertex_buffer[idx + 1u];
    // var v3 = vertex_buffer[idx + 2u];

    // v2.pos = (36 +  (36 << 16));
    // v1.pos = ((150 +  (86 << 16)));
    // v3.pos = (72 + (136 << 16));

    // draw_triangle(v1, v2, v3, local_invocation_id.xy, workgroup_id.x);
    // draw_anything(local_invocation_id.xy);

    // workgroupBarrier();
}

// 64 workgroups... (8x8)
@compute @workgroup_size(16,8,1)
fn clear_compose_buffer(
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

    let part = get_partition(local_invocation_id.xy, workgroup_id.xy);

    clear_layers(part);

    // if idx >= arrayLength(&vertex_buffer) {
    //     return;
    // }

    // now we have local_invocation_id.xy to play with...
    // var v1 = vertex_buffer[idx + 0u];
    // var v2 = vertex_buffer[idx + 1u];
    // var v3 = vertex_buffer[idx + 2u];

    // v2.pos = (36 +  (36 << 16));
    // v1.pos = ((150 +  (86 << 16)));
    // v3.pos = (72 + (136 << 16));

    // draw_triangle(v1, v2, v3, local_invocation_id.xy, workgroup_id.x);
    // draw_anything(local_invocation_id.xy);

    // workgroupBarrier();
}


