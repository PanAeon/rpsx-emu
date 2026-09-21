
// for now lazy path - just split whole 1024x512 drawing area, than think of something...
// but! we need to implement scissors anyway...
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

fn get_flag(data: Vertex, idx: u32) -> bool {
    return (data.texpage_base_and_flags & (u32(1) << (idx + 16))) != 0;
}

fn get_textured(data: Vertex) -> bool {
    return get_flag(data, 0);
}

fn is_semitrans(v: Vertex) -> bool {
    return get_flag(v, 1);
}

fn needs_blend(data: Vertex) -> bool {
    return get_flag(data, 2);
}

fn needs_dither(data: Vertex) -> bool {
    return get_flag(data, 3);
}
fn is_forced_set_mask_bit(data: Vertex) -> bool {
    return get_flag(data, 6);
}

fn is_preserve_masked_pixles(data: Vertex) -> bool {
    return get_flag(data, 7);
}
fn is_rectangle(data: Vertex) -> bool {
    return get_flag(data, 8);
}


fn get_depth(v: Vertex) -> u32 {
    return (v.color >> 24) & 0xFF;
}
fn get_texture_window(v: Vertex) -> vec4<i32> {
    let mask = v.texture_window & 0xFFFF;
    let offset = v.texture_window >> 16;

    let mask_x = mask & 0xFF;
    let mask_y = mask >> 8;
    let offset_x = offset & 0xFF;
    let offset_y = offset >> 8;

    return vec4(i32(mask_x), i32(mask_y), i32(offset_x), i32(offset_y));
}
fn get_texture_base(v: Vertex) -> vec2<i32> {
    let tex_base = v.texpage_base_and_flags & 0xFFFF;
    let tex_base_x = (tex_base & 0xFF) * 64;
    let tex_base_y = ((tex_base >> 8) & 0xFF) * 256;
    return vec2(i32(tex_base_x), i32(tex_base_y));
}

fn get_uv(v: Vertex) -> vec2<i32> {
    let x = v.uv & 0xFFFF;
    let y = (v.uv >> 16) & 0xFFFF;
    return vec2(i32(x), i32(y));
}

fn get_clut(v: Vertex) -> vec2<i32> {
    let x = v.clut & 0xFFFF;
    let y = (v.clut >> 16) & 0xFFFF;
    return vec2(i32(x), i32(y));
}
fn get_transparency(v: Vertex) -> u32 {
    let flags = v.texpage_base_and_flags >> 16;
    // transparency is bits 5,4
    let transparency = (flags >> 4) & 0x3;
    return transparency;
}

@group(0) @binding(0) var vram_t: texture_storage_2d<r32uint, read_write>;
@group(0) @binding(1) var<storage, read> vertex_buffer: array<Vertex>;
@group(0) @binding(2) var<storage, read> uniforms: Uniforms;
@group(0) @binding(3) var<storage, read_write> bins_buffer: array<u32>;

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

fn draw_rectangle(v1: Vertex, v2: Vertex, bounds: vec4<i32>) {
    let is_textured = get_textured(v1);
    // if is_textured { 
    //                     let p_ = pack_color(vec3(1.0, 0, 0), false);
    //                          textureStore(vram_t, vec2<u32>(u32(bounds.x), u32(bounds.y)), vec4(p_, 0, 0, 0));
    // }
    let p0 = get_pos(v1);
    let p1 = get_pos(v2);
    let rectangle_bounds = vec4<i32>(p0,p1);
    let bds = intersect(bounds, rectangle_bounds);
    let c0 = rgb8_split_color(v1.color);
    let uv0 = get_uv(v1);
    // let is_textured = get_textured(v1);
    let depth = get_depth(v1);
    let blend = needs_blend(v1);
    let dither = needs_dither(v1);
    let semi_trans = is_semitrans(v1);
    let force_set_mask_bit = is_forced_set_mask_bit(v1);
    let preserve_masked_pixels = is_preserve_masked_pixles(v1);

    let texture_window = get_texture_window(v1);
    let texpage_base = get_texture_base(v1);
    let clut = get_clut(v1);
    let transparency = get_transparency(v1);

    for (var y = bds.y; y <= bds.w; y++) {
        for (var x = bds.x; x <= bds.z; x++) {

            // let col = vec3<f32>(1.0, 0.0, 0.0);
            let col = c0;


                if is_textured {
                       //  let p_ = pack_color(vec3(1.0, 0, 0), false);
                       //       textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(p_, 0, 0, 0));
                       // continue;
                    var color: u32;
                    let raw_uv = vec2(uv0.x + x - p0.x, uv0.y + y - p0.y);
                    let uv = compute_uv_offset(raw_uv, texture_window);
                    switch depth {
                        case 0: {
                            let clut_idx = read_4bit(pack_h(texpage_base, 4) + uv);
                            let coord = vec2(clut.x + i32(clut_idx), clut.y);
                            let clut_color = read_16bit(coord);
                            color = clut_color;
                        }
                        case 1: {
                            let clut_idx = read_8bit(pack_h(texpage_base, 2) + uv);
                            let coord = vec2(clut.x + i32(clut_idx), clut.y);
                            color = read_16bit(coord);
                        }
                        default: {
                            color = read_16bit(texpage_base + uv);
                        }
                    }
                    if color == 0 {
                        // color = pack_color(vec3(1.0, 0, 0), false);
                        continue;
                    }

                    var c = unpack_color(color);

                    if blend {
                        let tex_color = vec3<u32>(vec3(c.r * 255, c.g * 255, c.b * 255));
                        let vert_color = vec3<u32>(col * 255);

                        let color_uint = min((tex_color * vert_color) >> vec3<u32>(7u), vec3<u32>(0xffu));
                        c = vec4<f32>(vec3<f32>(color_uint) / 255.0, c.w);
                    }

                    // if dither {
                    //     c *= 255;
                    //     var dither_pos: vec2<i32>;
                    //     dither_pos = vec2(x, y) % 4;
                    //     var dither_value: i32 = DITHER[u32(dither_pos.y)][u32(dither_pos.x)];
                    //     c.x += f32(dither_value);
                    //     c.y += f32(dither_value);
                    //     c.z += f32(dither_value);
                    //     c = clamp(c, vec4(0), vec4(0xff));
                    //     c /= 255;
                    // }

                    // if semi_trans && c.w > 0.5 {
                    if semi_trans && c.w > 0.5 {
                        let prev = readExistingColor(vec2(x, y));
                        c = blend_with_background(c, prev, transparency);
                    }

                    let pixel = pack_color(c.xyz, c.w > 0.5 || force_set_mask_bit);
                    if preserve_masked_pixels {
                        let prev = read_16bit(vec2(x, y));
                        if ((prev >> 15) & 0x1) != 1 {
                             textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                        }
                    } else {
                        textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                    }
                } else {
                    // continue;
                    var c = vec4<f32>(col, 0.0);
                    // if dither {
                    //     c *= 255;
                    //     var dither_pos: vec2<i32>;
                    //     dither_pos = vec2(x, y) % 4;
                    //     var dither_value: i32 = DITHER[u32(dither_pos.y)][u32(dither_pos.x)];
                    //     c.x += f32(dither_value);
                    //     c.y += f32(dither_value);
                    //     c.z += f32(dither_value);
                    //     c = clamp(c, vec4(0), vec4(0xff));
                    //     c /= 255;
                    // }
                    if semi_trans {
                        let prev = readExistingColor(vec2(x, y));
                        c = blend_with_background(c, prev, transparency);
                    }
                    let pixel = pack_color(c.xyz, force_set_mask_bit);
                    if preserve_masked_pixels {
                        let prev = read_16bit(vec2(x, y));
                        if ((prev >> 15) & 0x1) != 1 {
                             textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                        }
                    } else {
                         textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                    }
            }
        }
    }

}

fn draw_triangle_barycentric( v1: Vertex, v2: Vertex, v3: Vertex, bounds: vec4<i32>) {
    let p0 = get_pos(v1);
    let p1 = get_pos(v2);
    let p2 = get_pos(v3);

    let c0 = rgb8_split_color(v1.color);
    let c1 = rgb8_split_color(v2.color);
    let c2 = rgb8_split_color(v3.color);

    let uv0 = get_uv(v1);
    let uv1 = get_uv(v2);
    let uv2 = get_uv(v3);

    // let triangle_bounds = get_bounds(p0, p1, p2);
    // let bds = intersect(bounds, triangle_bounds);
    // if bds.x == -1 {
    //     return;
    // }
    let bds = bounds;

    let is_textured = get_textured(v1);
    let depth = get_depth(v1);
    let blend = needs_blend(v1);
    let dither = needs_dither(v1);
    let semi_trans = is_semitrans(v1);
    let force_set_mask_bit = is_forced_set_mask_bit(v1);
    let preserve_masked_pixels = is_preserve_masked_pixles(v1);

    let texture_window = get_texture_window(v1);
    let texpage_base = get_texture_base(v1);
    let clut = get_clut(v1);
    let transparency = get_transparency(v1);
// 0.0001

    for (var y = bds.y; y <= bds.w; y++) {
        for (var x = bds.x; x <= bds.z; x++) {
            let lambda = barycentric(p0, p1, p2, x, y);
            if lambda.x < 0.0000 || lambda.y < 0.0000 || lambda.z < 0.0000 {
                continue;
            }

            let col = interpolate_color(lambda, c0, c1, c2);

                // let col = vec3<f32>(
                //     f32(r_num / sum),
                //     f32(g_num / sum),
                //     f32(b_num / sum),
                // ) / 255.0;

                if is_textured {
                    var color: u32;
                    let uv = compute_uv_offset(interpolate_uv(lambda, uv0, uv1, uv2), texture_window);
                    switch depth {
                        case 0: {
                            let clut_idx = read_4bit(pack_h(texpage_base, 4) + uv);
                            let coord = vec2(clut.x + i32(clut_idx), clut.y);
                            let clut_color = read_16bit(coord);
                            color = clut_color;
                        }
                        case 1: {
                            let clut_idx = read_8bit(pack_h(texpage_base, 2) + uv);
                            let coord = vec2(clut.x + i32(clut_idx), clut.y);
                            color = read_16bit(coord);
                        }
                        default: {
                            color = read_16bit(texpage_base + uv);
                        }
                    }
                    if color == 0 {
                        continue;
                    }

                    var c = unpack_color(color);

                    if blend {
                        let tex_color = vec3<u32>(vec3(c.r * 255, c.g * 255, c.b * 255));
                        let vert_color = vec3<u32>(col * 255);

                        let color_uint = min((tex_color * vert_color) >> vec3<u32>(7u), vec3<u32>(0xffu));
                        c = vec4<f32>(vec3<f32>(color_uint) / 255.0, c.w);
                    }


                    // if semi_trans && c.w > 0.5 {
                    if semi_trans && c.w > 0.5 {
                        let prev = readExistingColor(vec2(x, y));
                        c = blend_with_background(c, prev, transparency);
                    }

                    if dither {
                        c *= 255;
                        var dither_pos: vec2<i32>;
                        dither_pos = vec2(x, y) % 4;
                        var dither_value: i32 = DITHER[u32(dither_pos.y)][u32(dither_pos.x)];
                        c.x += f32(dither_value);
                        c.y += f32(dither_value);
                        c.z += f32(dither_value);
                        c = clamp(c, vec4(0), vec4(0xff));
                        c /= 255;
                    }

                    let pixel = pack_color(c.xyz, c.w > 0.5 || force_set_mask_bit);
                    if preserve_masked_pixels {
                        let prev = read_16bit(vec2(x, y));
                        if ((prev >> 15) & 0x1) != 1 {
                             textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                        }
                    } else {
                        textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                    }
                } else {
                    var c = vec4<f32>(col, 0.0);
                    if semi_trans {
                        let prev = readExistingColor(vec2(x, y));
                        c = blend_with_background(c, prev, transparency);
                    }
                    if dither {
                        c *= 255;
                        var dither_pos: vec2<i32>;
                        dither_pos = vec2(x, y) % 4;
                        var dither_value: i32 = DITHER[u32(dither_pos.y)][u32(dither_pos.x)];
                        c.x += f32(dither_value);
                        c.y += f32(dither_value);
                        c.z += f32(dither_value);
                        c = clamp(c, vec4(0), vec4(0xff));
                        c /= 255;
                    }
                    let pixel = pack_color(c.xyz, force_set_mask_bit);
                    if preserve_masked_pixels {
                        let prev = read_16bit(vec2(x, y));
                        if ((prev >> 15) & 0x1) != 1 {
                             textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                        }
                    } else {
                         textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                    }
            }
        }
    }
}

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

    // let d = partial_derivatives[idx];
    // let r_dx = d.r_dx;
    // let g_dx = d.g_dx;
    // let b_dx = d.b_dx;
    //
    // let r_dy = d.r_dy;
    // let g_dy = d.g_dy;
    // let b_dy = d.b_dy;

    let r_dx = edge2.y * i32(c0.r) + edge3.y * i32(c1.r) + edge1.y * i32(c2.r);
    let g_dx = edge2.y * i32(c0.g) + edge3.y * i32(c1.g) + edge1.y * i32(c2.g);
    let b_dx = edge2.y * i32(c0.b) + edge3.y * i32(c1.b) + edge1.y * i32(c2.b);

    let r_dy = edge2.z * i32(c0.r) + edge3.z * i32(c1.r) + edge1.z * i32(c2.r);
    let g_dy = edge2.z * i32(c0.g) + edge3.z * i32(c1.g) + edge1.z * i32(c2.g);
    let b_dy = edge2.z * i32(c0.b) + edge3.z * i32(c1.b) + edge1.z * i32(c2.b);
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

fn draw_triangle_shaded_textured(v1: Vertex, v2: Vertex, v3: Vertex, bounds: vec4<i32>) {
    let p0 = get_pos(v1);
    let p1 = get_pos(v2);
    let p2 = get_pos(v3);

    let c0 = rgb8_split_color(v1.color) * 255;
    let c1 = rgb8_split_color(v2.color) * 255;
    let c2 = rgb8_split_color(v3.color) * 255;

    // let triangle_bounds = get_bounds(p0, p1, p2);
    // let bds = intersect(bounds, triangle_bounds);
    // if bds.x == -1 {
    //     return;
    // }
    let bds = bounds;

    let is_textured = get_textured(v1);
    let depth = get_depth(v1);
    let blend = needs_blend(v1);
    let dither = needs_dither(v1);
    let semi_trans = is_semitrans(v1);
    let force_set_mask_bit = is_forced_set_mask_bit(v1);
    let preserve_masked_pixels = false;//is_preserve_masked_pixles(v1);

    let texture_window = get_texture_window(v1);
    let texpage_base = get_texture_base(v1);
    let clut = get_clut(v1);
    let transparency = get_transparency(v1);

    let start = bds.xy;
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

    // should we add offset from start? (possible answer is yes)
    var u_row = edge2.x * uv0.x + edge3.x * uv1.x + edge1.x * uv2.x;
    var v_row = edge2.x * uv0.y + edge3.x * uv1.y + edge1.x * uv2.y;
    // var offset = triangle_bounds.xy - bounds.xy;
    // u_row += u_dx*offset.x + u_dy*offset.y;
    // v_row += v_dy*offset.y + v_dx*offset.x;

    var sum = edge1.x + edge2.x + edge3.x;
    if sum == 0 {
        sum = 1;
    }

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

                let col = vec3<f32>(
                    f32(r_num / sum),
                    f32(g_num / sum),
                    f32(b_num / sum),
                ) / 255.0;

                if is_textured {
                    var color: u32;
                    let uv = compute_uv_offset(vec2(u_num / sum, v_num / sum), texture_window);
                    switch depth {
                        case 0: {
                            let clut_idx = read_4bit(pack_h(texpage_base, 4) + uv);
                            let coord = vec2(clut.x + i32(clut_idx), clut.y);
                            let clut_color = read_16bit(coord);
                            color = clut_color;
                        }
                        case 1: {
                            let clut_idx = read_8bit(pack_h(texpage_base, 2) + uv);
                            let coord = vec2(clut.x + i32(clut_idx), clut.y);
                            color = read_16bit(coord);
                        }
                        default: {
                            color = read_16bit(texpage_base + uv);
                        }
                    }
                    if color == 0 {
                        continue;
                    }

                    var c = unpack_color(color);

                    if blend {
                        let tex_color = vec3<u32>(vec3(c.r * 255, c.g * 255, c.b * 255));
                        let vert_color = vec3<u32>(col * 255);

                        let color_uint = min((tex_color * vert_color) >> vec3<u32>(7u), vec3<u32>(0xffu));
                        c = vec4<f32>(vec3<f32>(color_uint) / 255.0, c.w);
                    }

                    // if dither {
                    //     c *= 255;
                    //     var dither_pos: vec2<i32>;
                    //     dither_pos = vec2(x, y) % 4;
                    //     var dither_value: i32 = DITHER[u32(dither_pos.y)][u32(dither_pos.x)];
                    //     c.x += f32(dither_value);
                    //     c.y += f32(dither_value);
                    //     c.z += f32(dither_value);
                    //     c = clamp(c, vec4(0), vec4(0xff));
                    //     c /= 255;
                    // }

                    // if semi_trans && c.w > 0.5 {
                    if semi_trans && c.w > 0.5 {
                        let prev = readExistingColor(vec2(x, y));
                        c = blend_with_background(c, prev, transparency);
                    }

                    let pixel = pack_color(c.xyz, c.w > 0.5 || force_set_mask_bit);
                    if preserve_masked_pixels {
                        let prev = read_16bit(vec2(x, y));
                        if ((prev >> 15) & 0x1) != 1 {
                             textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                        }
                    } else {
                        textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                    }
                } else {
                    var c = vec4<f32>(col, 0.0);
                    // if dither {
                    //     c *= 255;
                    //     var dither_pos: vec2<i32>;
                    //     dither_pos = vec2(x, y) % 4;
                    //     var dither_value: i32 = DITHER[u32(dither_pos.y)][u32(dither_pos.x)];
                    //     c.x += f32(dither_value);
                    //     c.y += f32(dither_value);
                    //     c.z += f32(dither_value);
                    //     c = clamp(c, vec4(0), vec4(0xff));
                    //     c /= 255;
                    // }
                    if semi_trans {
                        let prev = readExistingColor(vec2(x, y));
                        c = blend_with_background(c, prev, transparency);
                    }
                    let pixel = pack_color(c.xyz, force_set_mask_bit);
                    if preserve_masked_pixels {
                        let prev = read_16bit(vec2(x, y));
                        if ((prev >> 15) & 0x1) != 1 {
                             textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                        }
                    } else {
                         textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(pixel, 0, 0, 0));
                    }
                }
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

fn read_16bit(coord: vec2<i32>) -> u32 {
    var packed = textureLoad(vram_t, coord).r;
    return packed;
}

fn read_4bit(coord: vec2<i32>) -> u32 {
    var packed = read_16bit(vec2(coord.x / 4, coord.y));
    let bit_idx = u32(coord.x) % 4;
    let shift_amt = bit_idx * 4;

    return (packed >> shift_amt) & 0xFu;
}

fn read_8bit(coord: vec2<i32>) -> u32 {
    var packed = read_16bit(vec2(coord.x / 2, coord.y));
    let bit_idx = u32(coord.x) % 2;
    let shift_amt = bit_idx * 8;

    return (packed >> shift_amt) & 0xFFu;
}

fn unpack_color(v: u32) -> vec4<f32> {
    let r = (v & 31u);
    let g = ((v >> 5) & 31u);
    let b = ((v >> 10) & 31u);
    let a = ((v >> 15) & 1u);

    return vec4(
        FIVE_BIT_TO_8Bit[r],
        FIVE_BIT_TO_8Bit[g],
        FIVE_BIT_TO_8Bit[b],
        f32(a)
    );
}
fn readExistingColor(v: vec2<i32>) -> vec4<f32> {
    let color = read_16bit(v);
    return unpack_color(color);
}

fn pack_h(v: vec2<i32>, f: i32) -> vec2<i32> {
    return vec2<i32>(v.x * f, v.y);
}

fn compute_uv_offset(uv: vec2<i32>, texture_window: vec4<i32>) -> vec2<i32> {
    let x = u32(uv.x);
    let y = u32(uv.y);

    let x_mask = u32(texture_window.x) & 0xFF;//texture_window_mask[0]);
    let y_mask = u32(texture_window.y) & 0xFF;//u32(v.texture_window_mask[1]);
    let x_offset = u32(texture_window.z) & 0xFF;// u32(v.texture_window_offset[0]);
    let y_offset = u32(texture_window.w) & 0xFF;//u32(v.texture_window_offset[1]);

    return vec2<i32>(
        vec2(
            ((x & (~(x_mask * 8))) | ((x_offset & x_mask) * 8)) & 0xFFFF,
            ((y & (~(y_mask * 8))) | ((y_offset & y_mask) * 8)) & 0xFFFF)
    );
}


fn blend_with_background(color: vec4<f32>, prev: vec4<f32>, transparency: u32) -> vec4<f32> {
    var res: vec4<f32>;
    switch transparency {
        case 0: {
            res = clamp((color + prev) / vec4<f32>(2.0), vec4<f32>(0.0), vec4<f32>(1.0));
        }
        case 1: {
            res = clamp((color + prev), vec4<f32>(0.0), vec4<f32>(1.0));
        }
        case 2: {
            res = clamp((prev - color), vec4<f32>(0.0), vec4<f32>(1.0));
        }
        default: {
            res = clamp(prev + (color / vec4<f32>(4.0)), vec4<f32>(0.0), vec4<f32>(1.0));
        }
    }
    res.w = color.w;
    return res;
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

fn interpolate_color(lambda: vec3<f32>, c0: vec3<f32>, c1: vec3<f32>, c2: vec3<f32>) -> vec3<f32> {
    let r = min(1.0, (lambda[0] * c0[0] + lambda[1] * c1[0] + lambda[2] * c2[0]));
    let g = min(1.0, (lambda[0] * c0[1] + lambda[1] * c1[1] + lambda[2] * c2[1]));
    let b = min(1.0, (lambda[0] * c0[2] + lambda[1] * c1[2] + lambda[2] * c2[2]));
    return vec3<f32>(r, g, b);
}
fn interpolate_uv(lambda: vec3<f32>, uv0: vec2<i32>, uv1: vec2<i32>, uv2: vec2<i32>) -> vec2<i32> {
    let u = lambda[0] * f32(uv0[0]) + lambda[1] * f32(uv1[0]) + lambda[2] * f32(uv2[0]);
    let v = lambda[0] * f32(uv0[1]) + lambda[1] * f32(uv1[1]) + lambda[2] * f32(uv2[1]);
    return vec2<i32>(vec2(u, v));
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
    var width_bin_x = (uniforms.drawing_area_right - uniforms.drawing_area_left + 1) / 128;
    if width_bin_x * 128 < (uniforms.drawing_area_right - uniforms.drawing_area_left + 1) {
        width_bin_x++;
    }
    var width_bin_y = (uniforms.drawing_area_bottom - uniforms.drawing_area_top + 1) / 64;
    if width_bin_y * 64 < (uniforms.drawing_area_bottom - uniforms.drawing_area_top + 1) {
        width_bin_y++;
    }
    // let width_x = 1024u / 8;
    // let width_y = 512u / 8;
    return vec4<i32>(
        i32(id.x) * width_bin_x + uniforms.drawing_area_left-1,
        i32(id.y) * width_bin_y + uniforms.drawing_area_top-1,
        min(uniforms.drawing_area_right, (i32(id.x) + 1) * width_bin_x - 1 + uniforms.drawing_area_left-1),
        min(uniforms.drawing_area_bottom, (i32(id.y) + 1) * width_bin_y - 1 + uniforms.drawing_area_top-1),
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
    // if cx > dx || cy > dy {
    //     return vec4<i32>(-1);
    // }
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

    let bin_idx = (bin.y * 128 + bin.x);
    let num_vertices =  bins_buffer[bin_idx];
    let v_offset = bins_buffer[128*64 + bin_idx];
    let vert_offset = 2*(128*64) + v_offset;
    // let bounds = get_partition(bin);

    for (var i: u32 = 0; i < num_vertices; i++) {
        // let idx = (64 * (bin.y * 128 + bin.x)) + i;
        // let v_idx = bins_buffer[idx];
        let idx = vert_offset + i;
        let vert_idx = bins_buffer[idx];
        // if v_idx == 0 {
        //     continue;
        // }
        // bins_buffer[idx] = 0;
        // let vert_idx = v_idx - 1;
        var v1 = vertex_buffer[vert_idx + 0u];
        var v2 = vertex_buffer[vert_idx + 1u];
        var v3 = vertex_buffer[vert_idx + 2u];
        // draw_triangle_shaded_textured(v1, v2, v3, bounds);
        // if is_rectangle(v1) {
        //     draw_rectangle(v1, v2, bounds);
        // } else {
            draw_triangle_barycentric( v1, v2, v3, bounds);
            // draw_triangle_shaded_textured( v1, v2, v3, bounds);
        // }
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

    // let start = triangle_bounds.xy;
    // var edge1 = edge_function(start, p0, p1);// let (mut e1_row, a1, b1)
    // var edge2 = edge_function(start, p1, p2);
    // var edge3 = edge_function(start, p2, p0);
    //
    // let bias1 = i32(!is_top_left(p0, p1)); // could be precomputed
    // let bias2 = i32(!is_top_left(p1, p2));
    // let bias3 = i32(!is_top_left(p2, p0));
    //
    // let r_dx = edge2.y * i32(c0.r) + edge3.y * i32(c1.r) + edge1.y * i32(c2.r);
    // let g_dx = edge2.y * i32(c0.g) + edge3.y * i32(c1.g) + edge1.y * i32(c2.g);
    // let b_dx = edge2.y * i32(c0.b) + edge3.y * i32(c1.b) + edge1.y * i32(c2.b);
    //
    // let r_dy = edge2.z * i32(c0.r) + edge3.z * i32(c1.r) + edge1.z * i32(c2.r);
    // let g_dy = edge2.z * i32(c0.g) + edge3.z * i32(c1.g) + edge1.z * i32(c2.g);
    // let b_dy = edge2.z * i32(c0.b) + edge3.z * i32(c1.b) + edge1.z * i32(c2.b);
    //
    // let uv0 = get_uv(v1);
    // let uv1 = get_uv(v2);
    // let uv2 = get_uv(v3);
    //
    // let u_dx = edge2.y * uv0.x + edge3.y * uv1.x + edge1.y * uv2.x;
    // let v_dx = edge2.y * uv0.y + edge3.y * uv1.y + edge1.y * uv2.y;
    // let u_dy = edge2.z * uv0.x + edge3.z * uv1.x + edge1.z * uv2.x;
    // let v_dy = edge2.z * uv0.y + edge3.z * uv1.y + edge1.z * uv2.y;
    //
    // partial_derivatives[idx] = PartialDerivative(r_dx, g_dx, b_dx, r_dy, g_dy, b_dy, u_dx, v_dx, u_dy, v_dy);
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
@compute @workgroup_size(8,8,1)
fn quick_fill(
    @builtin(local_invocation_id) local_invocation_id: vec3<u32>, // actual ids of threads...
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local_invocation_index: u32,
    @builtin(num_workgroups) num_workgroups: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
) {
    let x = i32(workgroup_id.x * 8 + local_invocation_id.x);
    let y = i32(workgroup_id.y * 8 + local_invocation_id.y);

    if x >= uniforms.drawing_area_left && x <= uniforms.drawing_area_right &&
       y >= uniforms.drawing_area_top  && y <= uniforms.drawing_area_bottom {
       textureStore(vram_t, vec2<u32>(u32(x), u32(y)), vec4(uniforms.fill_color, 0, 0, 0));
    }
}
                // bins_buffer[64*(y*128 + x) + local_invocation_index] = idx;
const FIVE_BIT_TO_8Bit: array<f32, 32> = array(
    0,
    0.03137255,
    0.0627451,
    0.09803922,
    0.12941177,
    0.16078432,
    0.19215687,
    0.22745098,
    0.25882354,
    0.2901961,
    0.32156864,
    0.3529412,
    0.3882353,
    0.41960785,
    0.4509804,
    0.48235294,
    0.5176471,
    0.54901963,
    0.5803922,
    0.6117647,
    0.64705884,
    0.6784314,
    0.70980394,
    0.7411765,
    0.77254903,
    0.80784315,
    0.8392157,
    0.87058824,
    0.9019608,
    0.9372549,
    0.96862745,
    1,
);

const DITHER: array<array<i32, 4>, 4> = array(
    array(-4, 0, -3, 1),
    array(2, -2, 3, -1),
    array(-3, 1, -4, 0),
    array(3, -1, 2, -2),
);
