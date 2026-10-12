use std::{
    thread::{self, JoinHandle},
};

use crossbeam::channel::{Receiver, Sender};

use crate::{
    renderer::{ RendererMsg, RendererResponse},
};






#[allow(dead_code)]
pub struct DummyRenderer {
}

#[allow(dead_code)]
impl DummyRenderer {
    pub fn create(
    ) -> (
        Sender<RendererMsg>,
        Receiver<RendererResponse>,
        JoinHandle<()>,
    ) {
        let (_to_gpu_sender, gpu_receiver) = crossbeam::channel::bounded(1024);
        let (to_renderer_sender, receiver) = crossbeam::channel::bounded(1024);



        let handle = thread::spawn(move || {
            loop {
                match receiver.recv() {
                    Ok(_) => {},
                    Err(_) => return,
                };
            }
        });

        (to_renderer_sender, gpu_receiver, handle)
    }
}

