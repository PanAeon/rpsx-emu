struct TextureInfo {
    width: u32,
    height: u32,
}


// camera maps from world coordinates to NDC
@group(0) @binding(0) var<uniform> camera: mat4x4<f32>;

@group(1) @binding(0) var<uniform> texture_info: TextureInfo;
@group(1) @binding(1) var t_diffuse: texture_2d<f32>;
@group(1) @binding(2) var s_diffuse: sampler;


struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

const QUAD_VERTICES: array<vec4<f32>, 3> = array<vec4<f32>, 3>(
     vec4<f32>(0.0, -17.0, 0.0, 1.0),
     vec4<f32>(0.0, 17.0, 0.0, 1.0),
     vec4<f32>(17.0, 34.0, 0.0, 1.0),
);

const TEX_COORDS: array<vec2<f32>, 3> = array<vec2<f32>, 3>(
     vec2<f32>(0.0, 0.0),
     vec2<f32>(0.0, 17.0),
     vec2<f32>(17.0, 34.0),
);

@vertex
fn tilemap_vert_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var quad_vertices = QUAD_VERTICES;
    let position = quad_vertices[vertex_index % 3u]; 
    var ret: VertexOutput;
    // ret.position = tilemap.transform*position;
    ret.position = camera * position;
    ret.tex_coords = 
    // ret.position = camera * position;
    // let uvpos = ret.position.xy;
    let uvpos = position.xy;
    // let uvflip = vec2(uvpos.x, uvpos.y);
    let uvflip = vec2(uvpos.x, 1.0 - uvpos.y);
    let size_in_tiles = vec2<f32>(f32(60.0), f32(34.0));
    let size_in_pixels = vec2(16.0*60.0, 16.0*34);//size_in_tiles * vec2<f32>(size_of_tile);
    // ret.pos = vec2(2.0*(pos.x/(1.0+camera[3][0]))-2.0, 1.0  - pos.y)*size_in_tiles;
    let p = (ret.position+1.0)/2.0;
    let u = vec2(p.x, 1.0 - p.y);
    ret.pos = u * size_in_tiles;
    ret.tilepos = uvflip * size_in_tiles;
    ret.pixelpos = uvflip * size_in_pixels;
    return ret;
}

fn blend_layer(dst: vec4<f32>, data: VertexOtput, layer: u32) -> vec4<f32> { 
        var colorIdx: vec4<u32> = textureLoad(tilemap_indices, vec2<u32>(data.tilepos), layer, 0);
        var tileSize: u32 = 16;
        var tilex = u32(colorIdx.r)*tileSize;
        var tiley = u32(colorIdx.g)*tileSize;
        var px: u32 = (u32(data.pixelpos.x)) % tileSize;
        var py: u32 = (u32(data.pixelpos.y))% tileSize;;;//19;//u32(540.0*data.position.y/2048.0);
        var col: vec4<f32>  = textureLoad(tile_data, vec2<u32>(tilex + px, tiley + py), 0); // fuck
        var res = dst;
        if col.a != 0.0 {
            var asrc = col.a;
            var adst = dst.a;
            res = dst * dst.a * (1.0 - col.a) + col * col.a;
            res.a = asrc + adst * (1.0 - asrc);
            return res;
        }
        return dst;
}



@fragment
fn frag_main(data: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_diffuse, s_diffuse, in.tex_coords);
    // var res: vec4<f32> = vec4(0.0, 0.0, 0.0, 0.0);

    // if res.a != 0.0 {
    //    return res;
    // }
    // discard;
}
