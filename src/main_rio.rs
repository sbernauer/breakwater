use std::{
    io,
    net::{TcpListener},
};
use std::sync::Arc;
use crate::framebuffer::FrameBuffer;

mod framebuffer;

fn serve(ring: rio::Rio, acceptor: TcpListener, fb: Arc<FrameBuffer>) -> io::Result<()> {
    extreme::run(async move {
        loop {
            let stream = ring.accept(&acceptor).wait()?;

            let buf = vec![0_u8; 1024_000];
            loop {
                let bytes = ring.read_at(&stream, &buf, 0).await?;
                if bytes == 0 {
                    return Ok(());
                }

                let buffer = &buf[..bytes];
                //ring.write_at(b, &buf, 0).await?;
                //COUNT.fetch_add(read_bytes, std::sync::atomic::Ordering::Relaxed);


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
                                        // if buffer[i] == b'\n' {
                                        //     // i += 1;
                                        //     stream.write(rgba_to_hex_ascii_bytes(x, y, fb.get(x, y)).as_bytes()).unwrap();
                                        // }
                                    }
                                }
                            }
                        }
                    }
                }
            }


            // let mut buf = RESP;
            // while !buf.is_empty() {
            //     let written_bytes =
            //         ring.write_at(&stream, &buf, 0).await?;
            //     buf = &buf[written_bytes..];
            // }
        }
    })
}

fn main() -> io::Result<()> {
    let ring = rio::new()?;
    let acceptor = TcpListener::bind("127.0.0.1:6666")?;
    let fb = Arc::new(FrameBuffer::new(1280, 720));

    let mut threads = vec![];

    for _ in 0..12 {
        println!("Spawned thread");
        let acceptor = acceptor.try_clone().unwrap();
        let ring = ring.clone();
        let fb_clone = Arc::clone(&fb);
        threads.push(std::thread::spawn(move || {
            serve(ring, acceptor, fb_clone)
        }));
    }

    for thread in threads.into_iter() {
        thread.join().unwrap().unwrap();
    }

    Ok(())
}

fn from_hex_char(char: u8) -> u8 {
    match char {
        b'0'..=b'9' => char - b'0',
        b'a'..=b'f' => char - b'a' + 10,
        b'A'..=b'F' => char - b'A' + 10,
        _ => 0
    }
}