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

                                        // Performance over all :)
                                        // Also trying some branchless code, if you have any idea how to improve this try it out and please feel free to contact me!
                                        // We only parse the highest 4 bits of r, g and b.
                                        // Color format: 1 bit padding + 5 bit blue + 5 bit green + 5 bit red
                                        // As we would need to parse the second ASCII character to only get the 1 bit remaining to
                                        // the color depth of 5, we skip this and only use a color depth of 4 bit.
                                        let red: u8;
                                        if buffer[i - 9] > '0' as u8 - 1 && buffer[i - 9] < '9' as u8 + 1 {
                                            red = buffer[i - 9] - '0' as u8;
                                        } else {
                                            // If the char is A-Z lowercase it (Thanks to https://github.com/TobleMiner/shoreline for the idea)
                                            // Using existent buffer segment and not a new variable.
                                            // I don't know if this improves performance, but doing so anyway :)
                                            buffer[i - 9] = buffer[i - 9] | 0x20;

                                            if buffer[i - 9] > 'a' as u8 - 1 && buffer[i - 9] < 'f' as u8 + 1  {
                                                red = buffer[i - 9] - 'a' as u8 + 10;
                                            } else {
                                                continue;
                                            }
                                        }

                                        let green: u8;
                                        if buffer[i - 7] > '0' as u8 - 1 && buffer[i - 7] < '9' as u8 + 1 {
                                            green = buffer[i - 7] - '0' as u8;
                                        } else {
                                            // If the char is A-Z lowercase it (Thanks to https://github.com/TobleMiner/shoreline for the idea)
                                            // Using existent buffer segment and not a new variable.
                                            // I don't know if this improves performance, but doing so anyway :)
                                            buffer[i - 7] = buffer[i - 7] | 0x20;

                                            if buffer[i - 7] > 'a' as u8 - 1 && buffer[i - 7] < 'f' as u8 + 1  {
                                                green = buffer[i - 7] - 'a' as u8 + 10;
                                            } else {
                                                continue;
                                            }
                                        }

                                        let blue: u8;
                                        if buffer[i - 5] > '0' as u8 - 1 && buffer[i - 5] < '9' as u8 + 1 {
                                            blue = buffer[i - 5] - '0' as u8;
                                        } else {
                                            // If the char is A-Z lowercase it (Thanks to https://github.com/TobleMiner/shoreline for the idea)
                                            // Using existent buffer segment and not a new variable.
                                            // I don't know if this improves performance, but doing so anyway :)
                                            buffer[i - 5] = buffer[i - 5] | 0x20;

                                            if buffer[i - 5] > 'a' as u8 - 1 && buffer[i - 5] < 'f' as u8 + 1  {
                                                blue = buffer[i - 5] - 'a' as u8 + 10;
                                            } else {
                                                continue;
                                            }
                                        }

                                        let rgba: u32 = (blue as u32) << 20 | (green as u32) << 12 | (red as u32) << 4;
                                        // let rgba = 0b0111101111011110;

                                        //println!("For \"{}\" got : x={} y={} rgba={}", str::from_utf8(&buffer[loop_start..i]).unwrap(), x, y, rgba);
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

#[inline(always)]
fn rgba_to_hex_ascii_bytes(x: usize, y: usize, rgba: u32) -> String {
    format!("PX {x} {y} {:08x}\n", rgba)
}