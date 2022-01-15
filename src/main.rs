mod args;
mod framebuffer;
mod network;
mod vnc;

use clap::Parser;
use std::sync::Arc;
use std::thread;

use args::Args;
use framebuffer::FrameBuffer;

fn main() {
    let args = Args::parse();

    let fb = Arc::new(FrameBuffer::new(args.width, args.height));

    let fb_for_network = Arc::clone(&fb);
    let listen_address = args.listen_address.clone();
    let network_thread = thread::spawn(move || {
        network::listen(fb_for_network, listen_address.as_str());
    });

    thread::spawn(move || {
        vnc::start_vnc_server(&fb, args.fps);
    });

    network_thread.join().unwrap();
}
