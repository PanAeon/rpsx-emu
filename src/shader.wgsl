struct TextureInfo {
    width: u32,
    height: u32,
}

struct DisplayUniforms {
    is_24bpp: u32
}

// camera maps from world coordinates to NDC
@group(0) @binding(0) var<uniform> camera: mat4x4<f32>;
@group(0) @binding(1) var<uniform> uniforms: DisplayUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@group(1) @binding(0) var<uniform> texture_info: TextureInfo;
@group(1) @binding(1) var t_diffuse: texture_2d<u32>;
@group(1) @binding(2) var s_diffuse: sampler;


struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}


@vertex
fn vert_main(model: VertexInput) -> VertexOutput {
    var ret: VertexOutput;
    ret.tex_coords = model.uv;
    let position = vec4<f32>(model.position, 1.0);
    // ret.position = tilemap.transform*position;
    ret.clip_position = camera * position;
     // ret.clip_position =  position;
    // ret.tex_coords = TEX_COORDS[vertex_index % 6u];
    // ret.position = camera * position;
    // let uvpos = ret.position.xy;
    // let uvpos = position.xy;
    // // let uvflip = vec2(uvpos.x, uvpos.y);
    // let uvflip = vec2(uvpos.x, 1.0 - uvpos.y);
    // let size_in_tiles = vec2<f32>(f32(60.0), f32(34.0));
    // let size_in_pixels = vec2(16.0*60.0, 16.0*34);//size_in_tiles * vec2<f32>(size_of_tile);
    // // ret.pos = vec2(2.0*(pos.x/(1.0+camera[3][0]))-2.0, 1.0  - pos.y)*size_in_tiles;
    // let p = (ret.position+1.0)/2.0;
    // let u = vec2(p.x, 1.0 - p.y);
    // ret.pos = u * size_in_tiles;
    // ret.tilepos = uvflip * size_in_tiles;
    // ret.pixelpos = uvflip * size_in_pixels;
    return ret;
}

fn read_byte(byteOffset: u32, y: u32) -> u32 {
    let halfwordX = byteOffset / 2u;
    let halfword = textureLoad(t_diffuse, vec2(halfwordX, y), 0).r;
    // let halfword = texelFetch(vramRead, ivec2(halfwordX, y), 0).r;

    if ((byteOffset & 1u) == 1u) {
        return halfword >> 8u;
    }

    return halfword & 0xffu;
    // return textureLoad(t_diffuse, texcoord, 0).r;
}

fn read_24bit(coord: vec2<f32>) -> vec3<f32> {
    let texcoord = vec2<u32>(u32(coord.x), u32(coord.y));
    let byteOffset = /*displayStartX * 2u*/  texcoord.x * 3u;

    let r = read_byte(byteOffset + 0u, texcoord.y);
    let g = read_byte(byteOffset + 1u, texcoord.y);
    let b = read_byte(byteOffset + 2u, texcoord.y);

    return vec3(f32(r), f32(g), f32(b)) / 255.0;
}

fn read_16bit(coord: vec2<f32>) -> u32 {
    let texcoord = vec2<u32>(u32(coord.x), u32(coord.y));
    return textureLoad(t_diffuse, texcoord, 0).r;
}

fn rgb5_split_color(v: u32) -> vec3<f32> {
    let r = (v & 0x1F);
    let g = ((v >> 5) & 0x1F);
    let b = ((v >> 10) & 0x1F);

    return vec3(
        FIVE_BIT_TO_8Bit[r],
        FIVE_BIT_TO_8Bit[g],
        FIVE_BIT_TO_8Bit[b],
    );
    // let r = f32(v & 0x1F) / 31.0;
    // let g = f32((v >> 5) & 0x1F) / 31.0;
    // let b = f32((v >> 10) & 0x1F) / 31.0;
    // return vec3(r, g, b);
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

@fragment
fn frag_main(data: VertexOutput) -> @location(0) vec4<f32> {
    // let texcoord = vec2f(data.tex_coords.x, data.tex_coords.y);
    // return textureSample(t_diffuse, s_diffuse, texcoord);
    // return textureSample(t_diffuse, s_diffuse, data.tex_coords);
    // var res: vec4<f32> = vec4(0.0, 0.0, 0.0, 0.0);
    if uniforms.is_24bpp == 1 {
        var out = read_24bit(data.tex_coords);
        out.r = srgb_to_linear(out.r);
        out.g = srgb_to_linear(out.g);
        out.b = srgb_to_linear(out.b);

        return vec4<f32>(out, 1.0);
    } else {
        let col = read_16bit(data.tex_coords);

        var out = rgb5_split_color(col);

        out.r = srgb_to_linear(out.r);
        out.g = srgb_to_linear(out.g);
        out.b = srgb_to_linear(out.b);

        return vec4<f32>(out, 1.0);
    }

    // if res.a != 0.0 {
    //    return res;
    // }
    // discard;
}
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
