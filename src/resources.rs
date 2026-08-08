use std::io::{BufReader, Cursor};




pub async fn load_string(file_name: &str) -> anyhow::Result<String> {
    #[cfg(not(target_arch = "wasm32"))]
    let txt = {
        let path = std::path::Path::new("")
            .join("res")
            .join(file_name);
        std::fs::read_to_string(path)?
    };

    Ok(txt)
}

pub async fn load_binary(file_name: &str) -> anyhow::Result<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    let data = {
        let path = std::path::Path::new("")
            .join("res")
            .join(file_name);
        std::fs::read(path)?
    };

    Ok(data)
}

// pub async fn load_texture(
//     file_name: &str,
//     device: &wgpu::Device,
//     queue: &wgpu::Queue,
// ) -> anyhow::Result<texture::Texture> {
//     let data = load_binary(file_name).await?;
//     texture::Texture::from_bytes(device, queue, &data, file_name)
// }
 

