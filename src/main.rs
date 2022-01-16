mod args;
mod framebuffer;
mod network;
mod vnc;

use clap::Parser;
use std::sync::Arc;
use std::thread;

use args::Args;
use framebuffer::FrameBuffer;
use vnc::VncServer;

fn main() {
    let args = Args::parse();

    let fb = Arc::new(FrameBuffer::new(args.width, args.height));

    let fb_for_network = Arc::clone(&fb);
    let listen_address = args.listen_address.clone(); // TODO: Somehow avoid clone
    let network_thread = thread::spawn(move || {
        network::listen(fb_for_network, listen_address.as_str());
    });

    thread::spawn(move || {
        let vnc_text = format!("Brakewater @ {}", args.listen_address);
        let vnc_server = VncServer::new(&fb, args.fps, &vnc_text);
        vnc_server.run();
    });

    network_thread.join().unwrap();
}
