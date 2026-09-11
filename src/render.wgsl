

@group(0) @binding(0) var vram_t: texture_storage_2d<r32uint, read_write>;
// @group(1) @binding(2) var s_diffuse: sampler;

struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) color: u32,
    @location(2) uv: vec2<u32>,
    @location(3) flags: u32,
    @location(4) clut: vec2<u32>,
    @location(5) texpage_base: vec2<u32>,
    @location(6) texture_window_mask: vec2<u32>,
    @location(7) texture_window_offset: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @interpolate(linear) @location(1) vram_position: vec2<f32>,
    @interpolate(flat) @location(2) texture_depth: u32,
    @interpolate(flat) @location(3) clut: vec2<f32>,
    @interpolate(linear) @location(4) uv: vec2<f32>,
    @interpolate(flat) @location(5) texpage_base: vec2<f32>,
    @interpolate(flat) @location(6) flags: u32,
    @interpolate(flat) @location(7) texture_window_mask: vec2<u32>,
    @interpolate(flat) @location(8) texture_window_offset: vec2<u32>,
}

fn rgb8_split_color(value: u32) -> vec3<f32> {
    // unpack4x8unorm unpacks as
    // 0xrrggbb
    // we need
    // 0xbbggrr (r is lsb)
    return unpack4x8unorm(value).rgb;
}

@vertex
fn vert_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let color = in.color & 0x00FFFFFF;

    out.clip_position = vec4<f32>(f32(in.position.x) / 512.0 - 1.0, 1.0 - f32(in.position.y) / 256.0, 0.0, 1.0);
    // out.clip_position = vec4<f32>(f32(in.position.x), f32(in.position.y), 0.0, 1.0);
    out.vram_position = vec2<f32>(in.position);

    out.color = rgb8_split_color(color);
    out.texture_depth = (in.color >> 24u) & 0xFFu;
    out.flags = in.flags;

    out.clut = vec2<f32>(in.clut);

    out.uv = vec2<f32>(in.uv);
    out.texpage_base = vec2<f32>(vec2(u32(in.texpage_base.x) * 64, u32(in.texpage_base.y) * 256));

    out.texture_window_mask = in.texture_window_mask;
    out.texture_window_offset = in.texture_window_offset;

    // ret.tex_coords = model.uv;
    // let position = vec4<f32>(model.position, 1.0);
    // ret.clip_position = camera * position;
    return out;
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

fn pack_h(v: vec2<f32>, f: f32) -> vec2<f32> {
    return vec2<f32>(v.x * f, v.y);
}

fn compute_uv_offset(v: VertexOutput) -> vec2<f32> {
    let uv = vec2<u32>(v.uv);

    let x_mask = u32(v.texture_window_mask[0]);
    let y_mask = u32(v.texture_window_mask[1]);
    let x_offset = u32(v.texture_window_offset[0]);
    let y_offset = u32(v.texture_window_offset[1]);

    return vec2<f32>(
        vec2((uv[0] & (~(x_mask * 8u))) | ((x_offset & x_mask) * 8u),
            (uv[1] & (~(y_mask * 8u))) | ((y_offset & y_mask) * 8u))
    );
}

fn readExistingColor(v: VertexOutput) -> vec4<f32> {
    let color = read_16bit(v.vram_position);
    return unpack_color(color);
}

fn get_color(in: VertexOutput) -> u32 {
    if is_textured(in) {
        var color: u32;
        let uv = compute_uv_offset(in);
        switch in.texture_depth {
            case 0: {
                let clut_idx = read_4bit(pack_h(in.texpage_base, 4) + uv);
                let coord = vec2(in.clut.x + f32(clut_idx), in.clut.y);
                let clut_color = read_16bit(coord);
                color = clut_color;
            }
            case 1: {
                let clut_idx = read_8bit(pack_h(in.texpage_base, 2) + pack_h(uv, 1));
                let coord = vec2(in.clut.x + f32(clut_idx), in.clut.y);
                color = read_16bit(coord);
            }
            default: {
                color = read_16bit(pack_h(in.texpage_base, 1) + uv);
            }
        }
        if color == 0 {
            discard;
        }

        var c = unpack_color(color);

        if needs_blend(in) {
            let tex_color = vec3<u32>(vec3(c.r * 255, c.g * 255, c.b * 255));
            let vert_color = vec3<u32>(vec3(in.color.r * 255, in.color.g * 255, in.color.b * 255));

            let color_uint = min((tex_color * vert_color) >> vec3<u32>(7u), vec3<u32>(0xffu));
            c = vec4<f32>(vec3<f32>(color_uint) / 255.0, c.w);
        }

        if needs_dither(in) {
            c *= 255;
            var dither_pos: vec2<f32>;
            dither_pos = in.vram_position % 4;
            var dither_value: i32 = DITHER[u32(dither_pos.y)][u32(dither_pos.x)];
            c += f32(dither_value);
            c = clamp(c, vec4(0), vec4(0xff));
            c /= 255;
        }

        if is_semitrans(in) && c.w > 0.5 {
            let prev = readExistingColor(in);
            let transparency = get_transparency(in);
            c = blend_with_background(c, prev, transparency);
        }

        return pack_color(c.xyz, c.w > 0.5);
    } else {
        var c = vec4<f32>(in.color, 0.0);
        if needs_dither(in) {
            c *= 255;
            var dither_pos: vec2<f32>;
            dither_pos = in.vram_position % 4;
            var dither_value: i32 = DITHER[u32(dither_pos.y)][u32(dither_pos.x)];
            c += f32(dither_value);
            c = clamp(c, vec4(0), vec4(0xff));
            c /= 255;
        }
        if is_semitrans(in) {
            let prev = readExistingColor(in);
            let transparency = get_transparency(in);
            c = blend_with_background(c, prev, transparency);
        }
        return pack_color(c.xyz, false);
    }
}

fn blend_with_background(color: vec4<f32>, prev: vec4<f32>, transparency: u32) -> vec4<f32> {
    var res: vec4<f32>;
    switch transparency {
        case 0: {
            res = min((color + prev) / vec4<f32>(2.0), vec4<f32>(1.0));
        }
        case 1: {
            res = min((color + prev), vec4<f32>(1.0));
        }
        case 2: {
            res = max((prev - color), vec4<f32>(0.0));
        }
        default: {
            res = min(prev + (color / vec4<f32>(4.0)), vec4<f32>(1.0));
        }
    }
    res.w = color.w;
    return res;
}

fn get_flag(data: VertexOutput, idx: u32) -> bool {
    return (data.flags & (u32(1) << idx)) != 0;
}

fn is_textured(data: VertexOutput) -> bool {
    return get_flag(data, 0);
}

fn is_semitrans(v: VertexOutput) -> bool {
    return get_flag(v, 1);
}

fn needs_blend(data: VertexOutput) -> bool {
    return get_flag(data, 2);
}

fn needs_dither(data: VertexOutput) -> bool {
    return get_flag(data, 3);
}

fn get_transparency(v: VertexOutput) -> u32 {
    return ((v.flags >> 4) & 3u);
}

fn vramcoord_to_texcoord(coord: vec2<f32>) -> vec2<u32> {
    return vec2<u32>(vec2(coord.x, coord.y));
}

fn read_16bit(coord: vec2<f32>) -> u32 {
    let texcoord = vramcoord_to_texcoord(coord);
    // wgpu tex coords are +Y = down
    var packed = textureLoad(vram_t, texcoord).r;
    return packed;
    // return (packed >> ((u32(coord.x) % 2) * 16)) & 0xFFFF;
}

fn read_4bit(coord: vec2<f32>) -> u32 {
    var packed = read_16bit(vec2(coord.x / 4, coord.y));
    let bit_idx = u32(coord.x) % 4;
    let shift_amt = bit_idx * 4;

    return (packed >> shift_amt) & 0xFu;
}

fn read_8bit(coord: vec2<f32>) -> u32 {
    var packed = read_16bit(vec2(coord.x / 2, coord.y));
    let bit_idx = u32(coord.x) % 2;
    let shift_amt = bit_idx * 8;

    return (packed >> shift_amt) & 0xFFu;
}

@fragment
fn frag_main(data: VertexOutput) -> @location(0) u32 {
    // let texcoord = vec2f(data.tex_coords.x, data.tex_coords.y);
    // return textureSample(t_diffuse, s_diffuse, texcoord);
    // return textureSample(t_diffuse, s_diffuse, data.tex_coords);
    // var res: vec4<f32> = vec4(0.0, 0.0, 0.0, 0.0);

    // if res.a != 0.0 {
    //    return res;
    // }
    var color = get_color(data);
    if color == 0 {
        discard;
    }
    return color;
}

const DITHER: array<array<i32, 4>, 4> = array(
    array(-4, 0, -3, 1),
    array(2, -2, 3, -1),
    array(-3, 1, -4, 0),
    array(3, -1, 2, -2),
);

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
// const FIVE_BIT_TO_8Bit: array<u32, 32> = array(
// 0,
// 8,
// 16,
// 25,
// 33,
// 41,
// 49,
// 58,
// 66,
// 74,
// 82,
// 90,
// 99,
// 107,
// 115,
// 123,
// 132,
// 140,
// 148,
// 156,
// 165,
// 173,
// 181,
// 189,
// 197,
// 206,
// 214,
// 222,
// 230,
// 239,
// 247,
// 255,
// );
