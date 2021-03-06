use std::sync::Arc;
use std::thread;

use clap::Parser;

use breakwater::args::Args;
use breakwater::framebuffer::FrameBuffer;
use breakwater::network::Network;
use breakwater::statistics::{self, Statistics};
use breakwater::vnc::VncServer;

fn main() {
    let args = Args::parse();

    let fb = Arc::new(FrameBuffer::new(args.width, args.height));
    let statistics = if args.disable_statistics_save_file {
        Arc::new(Statistics::new(None))
    } else {
        Arc::new(Statistics::from_save_file_or_new(
            &args.statistics_save_file,
        ))
    };
    statistics::start_loop(Arc::clone(&statistics), args.statistics_save_interval_s);
    statistics::start_prometheus_server(args.prometheus_listen_address.as_str());

    let network_listen_address = args.listen_address.clone();
    let network_fb = Arc::clone(&fb);
    let network_statistics = Arc::clone(&statistics);
    let network_thread = thread::spawn(move || {
        let network = Network::new(&network_listen_address, network_fb, network_statistics);
        network.listen();
    });

    thread::spawn(move || {
        let vnc_text = args.text;
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
