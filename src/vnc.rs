use core::slice;
use std::thread;
use std::time::Duration;
use vncserver::*;

use crate::framebuffer::FrameBuffer;

pub fn start_vnc_server(fb: &FrameBuffer, fps: u32) {
    let vnc_server = initialize_vnc_server(fb);

    loop {
        for x in 0..fb.width {
            for y in 0..fb.height {
                // We don't use this as the wrapper method only exists for 16 bit, not for 32 bit :/
                // rfb_framebuffer_set_rgb16(vnc_server, x as i32, y as i32, fb.get(x, y));
                set_pixel(vnc_server, fb.width, fb.height, x, y, fb.get(x, y));
            }
        }
        rfb_mark_rect_as_modified(vnc_server, 0, 0, fb.width as i32, fb.height as i32);

        thread::sleep(Duration::from_millis(1000 / fps as u64)); // TODO Measure loop time and subtract it
    }

    // stop_vnc_server(vnc_server);
}

// We don't check for bounds as the only input is from this module
fn set_pixel(vnc_server: RfbScreenInfoPtr, width: usize,  height: usize, x: usize, y: usize, rgba: u32) {
    unsafe {
        let addr = (*vnc_server).frameBuffer as *mut u32;
        let slice: &mut [u32] = slice::from_raw_parts_mut(addr, width * height);
        slice[x + width * y] = rgba;
    }
}

fn initialize_vnc_server(fb: &FrameBuffer) -> RfbScreenInfoPtr {
    let vnc_server = rfb_get_screen(fb.width as i32, fb.height as i32, 5, 3, 4);
    rfb_framebuffer_malloc(vnc_server, (fb.width * fb.height * 4 /* bytes per pixel */) as u64);
    rfb_init_server(vnc_server);
    rfb_run_event_loop(vnc_server, 1, 1);

    vnc_server
}

// fn stop_vnc_server(vnc_server: RfbScreenInfoPtr) {
//     rfb_framebuffer_free(vnc_server);
//     rfb_screen_cleanup(vnc_server);
// }
