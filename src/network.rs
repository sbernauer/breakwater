use std::io::prelude::*;
use std::net::TcpListener;
use std::net::TcpStream;
use std::str;
use std::sync::Arc;
use std::thread;

use crate::framebuffer::FrameBuffer;
use crate::Statistics;

const NETWORK_BUFFER_SIZE: usize = 128_000;

pub struct Network<'a>{
    listen_address: &'a str,
    fb: Arc<FrameBuffer>,
    statistics: Arc<Statistics>,
}

impl<'a> Network<'a> {
    pub fn new(listen_address: &'a str, fb: Arc<FrameBuffer>, statistics: Arc<Statistics>) -> Self {
        Network{
            listen_address,
            fb,
            statistics
        }
    }

    pub fn listen(&self) {
        let listener = TcpListener::bind(self.listen_address)
            .expect(format!("Failed to listen on {}", self.listen_address).as_str());
        println!("Listening for Pixelflut connections on {}", self.listen_address);

        for stream in listener.incoming() {
            let stream = stream.unwrap();

            self.statistics.inc_connections(stream.peer_addr().unwrap().ip());

            let fb = Arc::clone(&self.fb);
            let statistics = Arc::clone(&self.statistics);
            thread::spawn(move || {
                handle_connection(stream, fb, statistics);
            });
        }
    }
}

fn handle_connection(mut stream: TcpStream, fb: Arc<FrameBuffer>, statistics: Arc<Statistics>) {
    let mut buffer = [0u8; NETWORK_BUFFER_SIZE];

    loop {
        let bytes = stream.read(&mut buffer).expect("Failed to read from stream");
        if bytes == 0 {
            statistics.dec_connections(stream.peer_addr().unwrap().ip());
            break;
        }

        let mut x: usize;
        let mut y: usize;

        let loop_lookahead = "PX 1234 1234 rrggbbaa".len();
        let loop_end;
        if bytes < loop_lookahead {
            loop_end = 0;
        } else {
            loop_end = bytes - loop_lookahead;
        }
        // We need to subtract XXX as the for loop can advance by max XXX bytes
        for mut i in 0..loop_end {
            if buffer[i] == b'P' {
                i += 1;
                if buffer[i] == b'X' {
                    i += 1;
                    if buffer[i] == b' ' {
                        i += 1;
                        // Parse first x coordinate char
                        if buffer[i] >= b'0' && buffer[i] <= b'9' {
                            x = (buffer[i] - b'0') as usize;
                            i += 1;

                            // Parse optional second x coordinate char
                            if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                x = 10 * x + (buffer[i] - b'0') as usize;
                                i += 1;

                                // Parse optional third x coordinate char
                                if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                    x = 10 * x + (buffer[i] - b'0') as usize;
                                    i += 1;

                                    // Parse optional forth x coordinate char
                                    if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                        x = 10 * x + (buffer[i] - b'0') as usize;
                                        i += 1;
                                    }
                                }
                            }

                            // Separator between x and y
                            if buffer[i] == b' ' {
                                i += 1;
                            }

                            // Parse first y coordinate char
                            if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                y = (buffer[i] - b'0') as usize;
                                i += 1;

                                // Parse optional second y coordinate char
                                if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                    y = 10 * y + (buffer[i] - b'0') as usize;
                                    i += 1;

                                    // Parse optional third y coordinate char
                                    if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                        y = 10 * y + (buffer[i] - b'0') as usize;
                                        i += 1;

                                        // Parse optional forth y coordinate char
                                        if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                            y = 10 * y + (buffer[i] - b'0') as usize;
                                            i += 1;
                                        }
                                    }
                                }

                                // Separator between coordinates and color
                                if buffer[i] == b' ' {
                                    i += 1;

                                    // Must be followed by 6 bytes RGB and newline or ...
                                    if buffer[i + 6] == b'\n' {
                                        i += 7;

                                        let rgba: u32 =
                                                  (from_hex_char(buffer[i - 3]) as u32) << 20
                                                | (from_hex_char(buffer[i - 2]) as u32) << 16
                                                | (from_hex_char(buffer[i - 5]) as u32) << 12
                                                | (from_hex_char(buffer[i - 4]) as u32) << 8
                                                | (from_hex_char(buffer[i - 7]) as u32) << 4
                                                | (from_hex_char(buffer[i - 6]) as u32) << 0;

                                        fb.set(x as usize, y as usize, rgba);

                                        continue;
                                    }

                                    // ... or must be followed by 8 bytes RGBA and newline
                                    if buffer[i + 8] == b'\n' {
                                        i += 9;

                                        let rgba: u32 =
                                                (from_hex_char(buffer[i - 5]) as u32) << 20
                                              | (from_hex_char(buffer[i - 4]) as u32) << 16
                                              | (from_hex_char(buffer[i - 7]) as u32) << 12
                                              | (from_hex_char(buffer[i - 6]) as u32) << 8
                                              | (from_hex_char(buffer[i - 9]) as u32) << 4
                                              | (from_hex_char(buffer[i - 8]) as u32) << 0;

                                        fb.set(x as usize, y as usize, rgba);

                                        continue;
                                    }
                                }

                                // End of command to read Pixel value
                                if buffer[i] == b'\n' {
                                    // i += 1;
                                    stream.write(rgba_to_hex_ascii_bytes(x, y, fb.get(x, y)).as_bytes()).unwrap();
                                }
                            }
                        }
                    }
                }
            }
        }
        stream.flush().unwrap(); // Flush return buffer e.g. for requested pixel values

        // println!("Got {} bytes ({}%))",
        //          bytes, bytes as f32 / BUFFER_SIZE as f32 * 100 as f32);
        //println!("{:?}", str::from_utf8(&buffer[0..64]).unwrap());
    }
}

fn rgba_to_hex_ascii_bytes(x: usize, y: usize, rgba: u32) -> String {
    format!("PX {x} {y} {:08x}\n", rgba)
}

fn from_hex_char(char: u8) -> u8 {
    match char {
        b'0'..=b'9' => char - b'0',
        b'a'..=b'f' => char - b'a' + 10,
        b'A'..=b'F' => char - b'A' + 10,
        _ => 0
    }
}
