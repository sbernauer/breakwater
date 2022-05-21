use std::io::prelude::*;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::str;
use std::sync::Arc;
use std::thread;

use crate::framebuffer::FrameBuffer;
use crate::statistics::Statistics;

const NETWORK_BUFFER_SIZE: usize = 256_000;
pub const HELP_TEXT: &[u8] = "\
Pixelflut server powered by breakwater https://github.com/sbernauer/breakwater
Available commands:
HELP: Show this help
PX x y rrggbb: Color the pixel (x,y) with the given hexadecimal color
PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb (alpha channel is ignored for now)
PX x y: Get the color value of the pixel (x,y)
SIZE: Get the size of the drawing surface, e.g. `SIZE 1920 1080`
OFFSET x y: Apply offset (x,y) to all further pixel draws on this connection
".as_bytes();
const LOOP_LOOKAHEAD: usize = "PX 1234 1234 rrggbbaa\n".len();

pub struct Network<'a> {
    listen_address: &'a str,
    fb: Arc<FrameBuffer>,
    statistics: Arc<Statistics>,
}

impl<'a> Network<'a> {
    pub fn new(listen_address: &'a str, fb: Arc<FrameBuffer>, statistics: Arc<Statistics>) -> Self {
        Network {
            listen_address,
            fb,
            statistics,
        }
    }

    pub fn listen(&self) {
        let listener = TcpListener::bind(self.listen_address)
            .unwrap_or_else(|err| panic!("Failed to listen on {}: {}", self.listen_address, err));
        println!(
            "Listening for Pixelflut connections on {}",
            self.listen_address
        );

        for stream in listener.incoming() {
            let stream = stream.expect("Failed to get tcp stream from listener");
            let ip = stream
                .peer_addr()
                .expect("Failed to get peer address from tcp connection")
                .ip();
            // If you connect via IPv4 you often show up as embedded inside an IPv6 address
            // Extracting the embedded information here, so we get the real (TM) address
            let ip = ip_to_canonical(ip);

            self.statistics.inc_connections(ip);

            let fb = Arc::clone(&self.fb);
            let statistics = Arc::clone(&self.statistics);
            thread::spawn(move || {
                handle_connection(stream, ip, fb, statistics);
            });
        }
    }
}

pub fn handle_connection(
    mut stream: impl Read + Write + Unpin,
    ip: IpAddr,
    fb: Arc<FrameBuffer>,
    statistics: Arc<Statistics>,
) {
    let mut buffer = [0u8; NETWORK_BUFFER_SIZE];
    // Number bytes left over **on the first bytes of the buffer** from the previous loop iteration
    let mut leftover_bytes_in_buffer = 0;

    let mut x: usize;
    let mut y: usize;
    let mut x_offset = 0;
    let mut y_offset = 0;

    loop {
        // Fill the buffer up with new data from the socket
        // If there are any bytes left over from the previous loop iteration leave them as is and but the new data behind
        let bytes = match stream.read(&mut buffer[leftover_bytes_in_buffer..]) {
            Ok(bytes) => bytes,
            Err(_) => {
                statistics.dec_connections(ip);
                break;
            }
        };

        statistics.inc_bytes(ip, bytes as u64);

        let mut loop_end = leftover_bytes_in_buffer + bytes;
        if bytes == 0 {
            if leftover_bytes_in_buffer == 0 {
                // We read no data and the previous loop did consume all data
                // Nothing to do here, closing connection
                statistics.dec_connections(ip);
                break;
            }

            // No new data from socket, read to the end and everything should be fine
            leftover_bytes_in_buffer = 0;
        } else {
            // Read some data, process it
            if loop_end >= NETWORK_BUFFER_SIZE {
                leftover_bytes_in_buffer = LOOP_LOOKAHEAD;
                loop_end -= leftover_bytes_in_buffer;
            } else {
                leftover_bytes_in_buffer = 0;
            }
        }

        let mut i = 0; // We can't use a for loop here because Rust don't lets use skip characters by incrementing i
        while i < loop_end {
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

                                    x += x_offset;
                                    y += y_offset;

                                    // Separator between coordinates and color
                                    if buffer[i] == b' ' {
                                        i += 1;

                                        // Must be followed by 6 bytes RGB and newline or ...
                                        if buffer[i + 6] == b'\n' {
                                            i += 7; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                                            let rgba: u32 = (from_hex_char(buffer[i - 3]) as u32)
                                                << 20
                                                | (from_hex_char(buffer[i - 2]) as u32) << 16
                                                | (from_hex_char(buffer[i - 5]) as u32) << 12
                                                | (from_hex_char(buffer[i - 4]) as u32) << 8
                                                | (from_hex_char(buffer[i - 7]) as u32) << 4
                                                | (from_hex_char(buffer[i - 6]) as u32);

                                            fb.set(x as usize, y as usize, rgba);
                                            if cfg!(feature = "count_pixels") {
                                                statistics.inc_pixels(ip);
                                            }
                                            continue;
                                        }

                                        // ... or must be followed by 8 bytes RGBA and newline
                                        if buffer[i + 8] == b'\n' {
                                            i += 9; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                                            let rgba: u32 = (from_hex_char(buffer[i - 5]) as u32)
                                                << 20
                                                | (from_hex_char(buffer[i - 4]) as u32) << 16
                                                | (from_hex_char(buffer[i - 7]) as u32) << 12
                                                | (from_hex_char(buffer[i - 6]) as u32) << 8
                                                | (from_hex_char(buffer[i - 9]) as u32) << 4
                                                | (from_hex_char(buffer[i - 8]) as u32);

                                            fb.set(x as usize, y as usize, rgba);
                                            if cfg!(feature = "count_pixels") {
                                                statistics.inc_pixels(ip);
                                            }
                                            continue;
                                        }
                                    }

                                    // End of command to read Pixel value
                                    if buffer[i] == b'\n' && x < fb.width && y < fb.height {
                                        match stream.write_all(
                                            format!(
                                                "PX {} {} {:06x}\n",
                                                // We don't want to return the actual (absolute) coordinates, the client should also get the result offseted
                                                x - x_offset,
                                                y - y_offset,
                                                fb.get(x, y).to_be() >> 8
                                            )
                                            .as_bytes(),
                                        ) {
                                            Ok(_) => (),
                                            Err(_) => continue,
                                        }
                                    }
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
                            stream
                                .write_all(format!("SIZE {} {}\n", fb.width, fb.height).as_bytes())
                                .expect("Failed to write bytes to tcp socket");
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
                            stream
                                .write_all(HELP_TEXT)
                                .expect("Failed to write bytes to tcp socket");
                        }
                    }
                }
            } else if buffer[i] == b'O'
                && buffer[i + 1] == b'F'
                && buffer[i + 2] == b'F'
                && buffer[i + 3] == b'S'
                && buffer[i + 4] == b'E'
                && buffer[i + 5] == b'T'
            {
                i += 6;
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

                                // End of command to set offset
                                if buffer[i] == b'\n' {
                                    x_offset = x;
                                    y_offset = y;
                                }
                            }
                        }
                    }
                }
            }

            i += 1;
        }

        if leftover_bytes_in_buffer > 0 {
            // We need to move the leftover bytes to the beginning of the buffer so that the next loop iteration con work on them
            buffer.copy_within(NETWORK_BUFFER_SIZE - leftover_bytes_in_buffer.., 0);
        }
    }
}

fn from_hex_char(char: u8) -> u8 {
    match char {
        b'0'..=b'9' => char - b'0',
        b'a'..=b'f' => char - b'a' + 10,
        b'A'..=b'F' => char - b'A' + 10,
        _ => 0,
    }
}

/// TODO: Switch to official ip.to_canonical() method when it is stable. **If** it gets stable sometime ;)
/// See <https://doc.rust-lang.org/std/net/enum.IpAddr.html#method.to_canonical>
fn ip_to_canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.octets() {
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d] => {
                IpAddr::V4(Ipv4Addr::new(a, b, c, d))
            }
            _ => ip,
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::network::*;
    use rstest::rstest;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[rstest]
    #[case(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), "0.0.0.0")]
    #[case(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), "127.0.0.1")]
    #[case(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)), "fe80::")]
    #[case(
        IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0x85a3, 0x0000, 0x0000, 0x8a2e, 0x0370, 0x7334)),
        "2001:db8:85a3::8a2e:370:7334"
    )]
    #[case(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xFFFF, 0, 1)), "0.0.0.1")]
    #[case(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xFFFF, 127 << 8, 1)), "127.0.0.1")]
    fn test_ip_to_string_respect_mapped_v4(#[case] input: IpAddr, #[case] expected: String) {
        assert_eq!(expected, ip_to_canonical(input).to_string());
    }
}
