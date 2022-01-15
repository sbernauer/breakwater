use std::thread;
use std::time::Duration;
use vncserver::*;

use crate::{WIDTH, HEIGHT};
use crate::framebuffer::FrameBuffer;

const FPS: u64 = 60;

pub fn start_vnc_server(fb: &FrameBuffer) {
    let vnc_server = initialize_vnc_server();

    loop {
        for x in 0..WIDTH {
            for y in 0..HEIGHT {
                rfb_framebuffer_set_rgb16(vnc_server, x as i32, y as i32, fb.get(x, y));
            }
        }
        rfb_mark_rect_as_modified(vnc_server, 0, 0, WIDTH as i32, HEIGHT as i32);

        thread::sleep(Duration::from_millis(1000 / FPS)); // TODO Measure loop time and subtract it
    }

    // stop_vnc_server(vnc_server);
}

fn initialize_vnc_server() -> RfbScreenInfoPtr {
    let vnc_server = rfb_get_screen(WIDTH as i32, HEIGHT as i32, 5, 3, 2);
    rfb_framebuffer_malloc(vnc_server, (WIDTH * HEIGHT * 2) as u64);
    rfb_init_server(vnc_server);
    rfb_run_event_loop(vnc_server, 1, 1);

    vnc_server
}

// fn stop_vnc_server(vnc_server: RfbScreenInfoPtr) {
//     rfb_framebuffer_free(vnc_server);
//     rfb_screen_cleanup(vnc_server);
// }