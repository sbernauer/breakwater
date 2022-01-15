use std::io::prelude::*;
use std::net::TcpListener;
use std::net::TcpStream;
use std::str;
use std::sync::Arc;
use std::thread;

use crate::framebuffer::FrameBuffer;

const NETWORK_BUFFER_SIZE: usize = 128_000;

pub fn listen(fb: Arc<FrameBuffer>, listen_address: &str) {
    let listener = TcpListener::bind(listen_address)
        .expect(format!("Failed to listen on {listen_address}").as_str());
    println!("Listening for Pixelflut connections on {listen_address}");

    for stream in listener.incoming() {
        let stream = stream.unwrap();

        let fb = Arc::clone(&fb);
        thread::spawn(move || {
            println!("Got connection from {}", stream.peer_addr().unwrap());
            handle_connection(stream, fb);
        });
    }
}

pub fn handle_connection(mut stream: TcpStream, fb: Arc<FrameBuffer>) {
    let mut buffer = [0u8; NETWORK_BUFFER_SIZE];

    loop {
        let bytes = stream.read(&mut buffer).expect("Failed to read from stream");
        if bytes == 0 {
            println!("Got connection from {}", stream.peer_addr().unwrap());
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
            let loop_start = i;
            if buffer[i] == 'P' as u8 {
                i += 1;
                if buffer[i] == 'X' as u8 {
                    i += 1;
                    if buffer[i] == ' ' as u8 {
                        i += 1;
                        // Parse first x coordinate char
                        if buffer[i] > '/' as u8 && buffer[i] < ':' as u8 {
                            x = (buffer[i] - '0' as u8) as usize;
                            i += 1;

                            // Parse optional second x coordinate char
                            if buffer[i] > '/' as u8 && buffer[i] < ':' as u8 {
                                x = 10 * x + (buffer[i] - '0' as u8) as usize;
                                i += 1;

                                // Parse optional third x coordinate char
                                if buffer[i] > '/' as u8 && buffer[i] < ':' as u8 {
                                    x = 10 * x + (buffer[i] - '0' as u8) as usize;
                                    i += 1;

                                    // Parse optional forth x coordinate char
                                    if buffer[i] > '/' as u8 && buffer[i] < ':' as u8 {
                                        x = 10 * x + (buffer[i] - '0' as u8) as usize;
                                        i += 1;
                                    }
                                }
                            }

                            // Separator between x and y
                            if buffer[i] == ' ' as u8 {
                                i += 1;
                            }

                            // Parse first y coordinate char
                            if buffer[i] > '/' as u8 && buffer[i] < ':' as u8 {
                                y = (buffer[i] - '0' as u8) as usize;
                                i += 1;

                                // Parse optional second y coordinate char
                                if buffer[i] > '/' as u8 && buffer[i] < ':' as u8 {
                                    y = 10 * y + (buffer[i] - '0' as u8) as usize;
                                    i += 1;

                                    // Parse optional third y coordinate char
                                    if buffer[i] > '/' as u8 && buffer[i] < ':' as u8 {
                                        y = 10 * y + (buffer[i] - '0' as u8) as usize;
                                        i += 1;

                                        // Parse optional forth y coordinate char
                                        if buffer[i] > '/' as u8 && buffer[i] < ':' as u8 {
                                            y = 10 * y + (buffer[i] - '0' as u8) as usize;
                                            i += 1;
                                        }
                                    }
                                }

                                // Separator between coordinates and color
                                if buffer[i] == ' ' as u8 {
                                    i += 1;

                                    // Must be followed by 6 bytes RGB and newline or ...
                                    if buffer[i + 6] == '\n' as u8 {
                                        i += 7;
                                        println!("For \"{}\" got : x={} y={}", str::from_utf8(&buffer[loop_start..i]).unwrap(), x, y);
                                        continue;
                                    }

                                    // ... or must be followed by 8 bytes RGBA and newline
                                    if buffer[i + 8] == '\n' as u8 {
                                        // Advancing early as we can continue the loop if non hey charater follows
                                        // In this case we can skip some already parsed bytes
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
                                if buffer[i] == '\n' as u8 {
                                    i += 1;
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