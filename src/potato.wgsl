

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@group(0) @binding(0) var t_diffuse: texture_2d<f32>;
@group(0) @binding(1) var s_diffuse: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

const CAMERA: mat4x4<f32> = mat4x4<f32>(
    2.0, 0.0, 0.0, 0.0,
    0.0, 2.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0,
    -1.0, -1.0, 0.0, 1.0
);

@vertex
fn vert_main(model: VertexInput) -> VertexOutput {
    var ret: VertexOutput;
    ret.tex_coords = model.uv;
    let position = vec4<f32>(model.position, 1.0);
    // ret.position = tilemap.transform*position;
    ret.position = CAMERA * position;
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

// @fragment
// fn frag_main(in: VertexOutput) -> @location(0) vec4<f32> {
//     let texcoord = vec2f(in.tex_coords.x, 1.0 - in.tex_coords.y);
//     // return textureSample(t_diffuse, s_diffuse, texcoord);
//     // return textureSample(t_diffuse, s_diffuse, data.tex_coords);
//     // var res: vec4<f32> = vec4(0.0, 0.0, 0.0, 0.0);
//     let banding = abs(sin(in.position.y * bandMult));
//
//     let cellNdx = u32(in.position.x * cellMult) % 3;
//     var cellColor = vec3f(0);
//     cellColor[cellNdx] = 1.0;
//     let cMult = cellColor + cellBright;
//
//     let effect = mix(vec3f(1), banding * cMult, effectAmount);
//     let color = textureSample(t_diffuse, s_diffuse, texcoord);
//     return vec4f(color.rgb * effect, color.a);
//
//     // if res.a != 0.0 {
//     //    return res;
//     // }
//     // discard;
// }

// const effectAmount: f32 = 1.0;
// const bandMult: f32 = 1.0;
// const cellMult: f32 = 0.5;
// const cellBright: f32 = 1.0;

      // @group(0) @binding(0) var postTexture2d: texture_2d<f32>;
      // @group(0) @binding(1) var postSampler: sampler;

      // @fragment fn fs2d(in: VertexOutput) -> @location(0) vec4f {
// }

/// ------------------------- 

// Uniform layout matching standard WebGPU buffer structures
// struct CRTUniforms {
//     time: f32,
//     curvature: f32,
//     scanline_intensity: f32,
//     vignette_intensity: f32,
// };
const curvature: f32 = 0.15;  
const scanline_intensity: f32 = 0.25;
const vignette_intensity: f32 = 0.10;

// @group(0) @binding(0) var textureSampler: sampler;
// @group(0) @binding(1) var inputTexture: texture_2d<f32>;
// @group(0) @binding(2) var<uniform> uniforms: CRTUniforms;

// --- Vertex Shader ---
// Generates a full-screen triangle natively without needing vertex buffers
// @vertex
// fn vs_main(@builtin(vertex_index) vertexIndex: u32) -> VertexOutput {
//     var output: VertexOutput;
//     var uv = vec2f(
//         f32((vertexIndex << 1u) & 2u),
//         f32(vertexIndex & 2u)
//     );
//     output.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
//     // Invert Y for correct web texture coordination space
//     output.tex_coords = vec2f(uv.x, 1.0 - uv.y);
//     return output;
// }

// --- Helper Functions ---
// Distorts the UV coordinates to create a fish-eye bulge effect
fn distort_uv(uv: vec2f, curvature: f32) -> vec2f {
    // Transform coordinates from [0, 1] to [-1, 1]
    var cc = uv - 0.5;
    var dist = dot(cc, cc);
    
    // Apply radial distortion formula
    cc = cc * (1.0 + dist * curvature * (1.0 + dist * curvature));
    
    // Restore back to original [0, 1] texture coordinates
    return cc + 0.5;
}

// --- Fragment Shader ---
@fragment
fn frag_main(in: VertexOutput) -> @location(0) vec4f {
    let uv = vec2f(in.tex_coords.x, 1.0 - in.tex_coords.y);
    // 1. Curvature
    var distortedUV = distort_uv(uv, curvature);
    
    // Hard cutoff clip for pixels pushed outside the simulated screen boundaries
    if (distortedUV.x < 0.0 || distortedUV.x > 1.0 || distortedUV.y < 0.0 || distortedUV.y > 1.0) {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    
    // 2. Sample Base Texture
    var color = textureSample(t_diffuse, s_diffuse, distortedUV);
    
    // 3. Scanline Emulation
    // Modulate using a sine wave synced to the vertical axis and dynamic time drift
    // var scanline = sin(distortedUV.y * 800.0 + uniforms.time * 2.0);
    var scanline = sin(distortedUV.y * 800.0);
    // Lerp between raw color and dark scanline gaps based on intensity uniform
    color = mix(color, color * (0.5 + 0.5 * scanline), scanline_intensity);
    
    // 4. Phosphor Mask (Trinitron-style fine horizontal grill lines)
    var mask = 0.8 + 0.2 * sin(distortedUV.x * 1200.0);
    color = color * mask;
    
    // 5. Vignette (Darkening towards the vintage tube corners)
    var vign = distortedUV.x * distortedUV.y * (1.0 - distortedUV.x) * (1.0 - distortedUV.y);
    vign = clamp(pow(16.0 * vign, vignette_intensity), 0.0, 1.0);
    color = color * vign;
    
    return vec4f(color.rgb, 1.0);
}
