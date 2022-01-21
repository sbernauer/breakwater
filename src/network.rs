use std::io::prelude::*;
use std::net::TcpListener;
use std::net::TcpStream;
use std::str;
use std::sync::Arc;
use std::thread;

use crate::framebuffer::FrameBuffer;
use crate::Statistics;

const NETWORK_BUFFER_SIZE: usize = 128_000;
const HELP_TEXT: &[u8] = "\
Pixelflut server powered by Breakwater https://github.com/sbernauer/breakwater
Available commands:
HELP: Show this help
PX x y rrggbb: Color the pixel (x,y) with the given hexadecimal color
PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb (alpha channel is ignored for now)
PX x y: Get the color value of the pixel (x,y)
SIZE: Get the size of the drawing surface
".as_bytes();

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
    let ip = stream.peer_addr().unwrap().ip();
    let mut buffer = [0u8; NETWORK_BUFFER_SIZE];
    let mut output_written = false;

    loop {
        let bytes = stream.read(&mut buffer).expect("Failed to read from stream");
        statistics.inc_bytes(ip, bytes as u64);
        if bytes == 0 {
            statistics.dec_connections(ip);
            break;
        }

        let mut x: usize;
        let mut y: usize;

        let loop_lookahead = "PX 1234 1234 rrggbbaa\n".len();
        let mut loop_end = bytes;
        if bytes + loop_lookahead >= NETWORK_BUFFER_SIZE {
            loop_end -= loop_lookahead;
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
                                    i += 1;
                                    stream.write(format!("PX {x} {y} {:06x}\n", fb.get(x, y).to_be() >> 8).as_bytes()).unwrap();
                                    output_written = true;
                                }
                            }
                        }
                    }
                }
            } else if buffer[i] == b'S' {
                i += 1;
                if buffer[i] == b'I' {
                    i += 1;
                    if buffer[i] == b'Z' {
                        i += 1;
                        if buffer[i] == b'E' {
                            i += 1;
                            stream.write(format!("SIZE {} {}\n", fb.width, fb.height).as_bytes()).unwrap();
                            output_written = true;
                        }
                    }
                }
            } else if buffer[i] == b'H' {
                i += 1;
                if buffer[i] == b'E' {
                    i += 1;
                    if buffer[i] == b'L' {
                        i += 1;
                        if buffer[i] == b'P' {
                            i += 1;
                            stream.write(HELP_TEXT).unwrap();
                            output_written = true;
                        }
                    }
                }
            }
        }
        if output_written {
            stream.flush().unwrap();
        }
    }
}

fn from_hex_char(char: u8) -> u8 {
    match char {
        b'0'..=b'9' => char - b'0',
        b'a'..=b'f' => char - b'a' + 10,
        b'A'..=b'F' => char - b'A' + 10,
        _ => 0
    }
}
