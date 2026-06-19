#!/usr/bin/env -S cargo +nightly -Zscript
---
package.edition = "2024"

[profile.dev]
opt-level = 3
---

use std::env::args;
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::slice;

const DATA_PRELUDE: &[u8; 16] = b"%%KFSVS%%\xaa\xbb\xcc\xdd\xee\xff\0";
const ENDIAN_MARKER: u32 =
    cfg_select! {target_endian = "big" => 0u32, target_endian = "little" => 1u32};

pub fn main() {
    let listen_addr: SocketAddr = args()
        .nth(1)
        .expect("usage: kfsvs-receive LISTEN_ADDR:LISTEN_PORT")
        .parse()
        .expect("invalid socket address");

    let socket = UdpSocket::bind(listen_addr).expect("could not bind to UDP socket");
    let mut buffer = vec![0; 24];

    let mut stdout = std::io::stdout();

    let mut frame = Vec::new();
    'frame: loop {
        let (size, remote_addr) = socket.recv_from(buffer.as_mut_slice()).unwrap();
        let data = &buffer[..size];

        if data.starts_with(DATA_PRELUDE) && data.len() >= 24 {
            // Correct new header received.
            let frame_size = u32::from_be_bytes(*data[16..20].as_array().unwrap());
            let endianness = u32::from_be_bytes(*data[20..24].as_array().unwrap());
            eprintln!(
                "received frame of size {frame_size} endianness {endianness} from {remote_addr:?}"
            );
            if frame_size == 0 {
                continue;
            }

            if !frame_size.is_multiple_of(4) {
                eprintln!("error: frame size is not multiple of 4");
                continue;
            }

            // Receive rest of frame.
            frame.resize((frame_size / 4) as usize, 0u32);
            // SAFETY: idk, things are broken sometimes
            let raw_frame = unsafe {
                slice::from_raw_parts_mut(frame.as_mut_ptr() as *mut u8, frame.len() * 4)
            };
            let mut write_index = 0;
            if data.len() > 24 {
                raw_frame.copy_from_slice(&data[24..]);
                write_index += data.len() - 24;
            }

            while write_index < frame_size as usize {
                let (size, _) = socket.recv_from(&mut raw_frame[write_index..]).unwrap();
                // This condition fixes all visual glitches, even when it doesn’t trigger.
                if raw_frame[write_index..].starts_with(b"%%KF") {
                    eprintln!("transport got messed up, discarding frame");
                    continue 'frame;
                }
                write_index += size;
            }
            // eprintln!("finished receiving frame");

            // Send out frame over stdout
            for pixel in &frame {
                let correct_endianness_pixel = if endianness != ENDIAN_MARKER {
                    pixel.swap_bytes()
                } else {
                    *pixel
                };
                stdout
                    .write_all(&correct_endianness_pixel.to_le_bytes()[0..3])
                    .expect("ffmpeg exited");
            }
            stdout.flush().unwrap();
        }
    }
}
