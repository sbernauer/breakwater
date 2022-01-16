use core::slice;
use std::thread;
use std::time::Duration;
use vncserver::*;

use crate::framebuffer::FrameBuffer;

pub struct VncServer<'a> {
    fb: &'a FrameBuffer,
    screen: RfbScreenInfoPtr,
    fps: u32,
}

impl<'a> VncServer<'a> {
    pub fn new(fb: &'a FrameBuffer, fps: u32) -> Self {
        let screen = rfb_get_screen(fb.width as i32, fb.height as i32, 8, 3, 4);
        rfb_framebuffer_malloc(screen, (fb.width * fb.height * 4 /* bytes per pixel */) as u64);
        rfb_init_server(screen);
        rfb_run_event_loop(screen, 1, 1);

        VncServer{
            fb,
            screen,
            fps
        }
    }

    pub fn run(&self) {
        loop {
            for x in 0..self.fb.width {
                for y in 0..self.fb.height {
                    // We don't use this as the wrapper method only exists for 16 bit, not for 32 bit :/
                    // rfb_framebuffer_set_rgb16(vnc_server, x as i32, y as i32, fb.get(x, y));
                    self.set_pixel(x, y, self.fb.get(x, y));
                }
            }
            rfb_mark_rect_as_modified(self.screen, 0, 0, self.fb.width as i32, self.fb.height as i32);

            thread::sleep(Duration::from_millis(1000 / self.fps as u64)); // TODO Measure loop time and subtract it
        }
    }

    // We don't check for bounds as the only input is from this struct
    fn set_pixel(&self, x: usize, y: usize, rgba: u32) {
        unsafe {
            let addr = (*self.screen).frameBuffer as *mut u32;
            let slice: &mut [u32] = slice::from_raw_parts_mut(addr, self.fb.width * self.fb.height);
            slice[x + self.fb.width * y] = rgba;
        }
    }
}
