use std::sync::Arc;
use std::thread;

use clap::Parser;

use crate::args::Args;
use crate::framebuffer::FrameBuffer;
use crate::network::Network;
use crate::statistics::Statistics;
use crate::vnc::VncServer;

mod args;
mod framebuffer;
mod network;
mod statistics;
mod test;
mod vnc;

fn main() {
    let args = Args::parse();

    let fb = Arc::new(FrameBuffer::new(args.width, args.height));
    let statistics = Arc::new(Statistics::new());
    statistics::start_loop(Arc::clone(&statistics));
    statistics::start_prometheus_server(args.prometheus_listen_address.as_str());

    let network_listen_address = args.listen_address.clone();
    let network_fb = Arc::clone(&fb);
    let network_statistics = Arc::clone(&statistics);
    let network_thread = thread::spawn(move || {
        let network = Network::new(&network_listen_address, network_fb, network_statistics);
        network.listen();
    });

    thread::spawn(move || {
        let vnc_text = format!("{} on {}", args.text, args.listen_address);
        let vnc_server = VncServer::new(
            &fb,
            args.vnc_port,
            args.fps,
            &vnc_text,
            &statistics,
            &args.font,
        );
        vnc_server.run();
    });

    network_thread
        .join()
        .expect("Failed to join network thread");
}
