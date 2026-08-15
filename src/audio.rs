use std::time::Duration;

use cpal::{StreamConfig, traits::{DeviceTrait, HostTrait}};
use crossbeam::channel::Sender;
use anyhow::Context;


pub type AudioSample = [i16;2];


const AUDIO_STREAM_CONFIG: StreamConfig = StreamConfig {
    channels: 2,
    sample_rate: 44100_u32,
    buffer_size: cpal::BufferSize::Fixed(1024),
};

pub fn build_audio_stream() -> anyhow::Result<(cpal::Stream, Sender<AudioSample>)> {
    let (prod, cons) = crossbeam::channel::bounded(4096);

    let device = cpal::host_from_id(cpal::HostId::PipeWire)
        .expect("fail to open audio device")
        .default_output_device()
        .context("no output device available")?;

    // let mut last_sample: [i16;2] = [0,0];
    let stream = device.build_output_stream(
            AUDIO_STREAM_CONFIG,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                // println!("data len: {}", data.len());
                for d in data.as_chunks_mut::<2>().0 {
                    // match cons.try_recv() {
                    //     Ok(sample) => *d = sample,
                    //     Err(_) => {
                    //         println!("<audio buffer underrun>");
                    //         std::thread::sleep(std::time::Duration::from_millis(400));
                    //         return;
                    //     },
                    // };
                    *d = cons.recv().unwrap_or_default();
                    // *d = cons.try_recv().unwrap_or(last_sample);
                    // *d = cons.recv_timeout(Duration::from_micros(1)).unwrap_or(last_sample);
                    // last_sample = *d;
                }
            },
            move |err| println!("an error occurred on the output audio stream {err}"),
            None)?;
    Ok((stream, prod))
}
