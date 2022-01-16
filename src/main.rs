mod args;
mod framebuffer;
mod network;
mod vnc;
mod statistics;

use clap::Parser;
use std::sync::Arc;
use std::thread;

use args::Args;
use framebuffer::FrameBuffer;
use vnc::VncServer;
use statistics::Statistics;
use network::Network;

fn main() {
    let args = Args::parse();

    let fb = Arc::new(FrameBuffer::new(args.width, args.height));
    let statistics = Arc::new(Statistics::new());

    let network_listen_address = args.listen_address.clone(); // TODO: Somehow avoid clone
    let network_fb = Arc::clone(&fb);
    let network_statistics = Arc::clone(&statistics);
    let network_thread = thread::spawn(move || {
        let network = Network::new(network_listen_address.as_str(), network_fb, network_statistics);
        network.listen();
    });

    thread::spawn(move || {
        let vnc_text = format!("Brakewater @ {}", args.listen_address);
        let vnc_server = VncServer::new(&fb, args.fps, &vnc_text, &statistics);
        vnc_server.run();
    });

    network_thread.join().unwrap();
}
