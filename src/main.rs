mod framebuffer;
mod network;
mod vnc;

use std::sync::Arc;
use std::thread;

use framebuffer::FrameBuffer;

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;

fn main() {
    let fb = Arc::new(FrameBuffer::new());

    let fb_for_network = Arc::clone(&fb);
    let network_thread = thread::spawn(move || {
        network::listen(fb_for_network);
    });

    thread::spawn(move || {
        vnc::start_vnc_server(&fb);
    });

    network_thread.join().unwrap();
}
