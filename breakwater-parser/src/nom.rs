use core::str;
use std::{num::ParseIntError, sync::Arc};

use nom::{
    branch::alt,
    bytes::complete::{tag, take_while_m_n},
    character::complete::char,
    combinator::{map, map_res},
    sequence::{preceded, separated_pair, terminated},
    Finish, IResult,
};

use crate::{FrameBuffer, Parser, HELP_TEXT};

pub struct NomParser<FB: FrameBuffer> {
    fb: Arc<FB>,

    connection_x_offset: u16,
    connection_y_offset: u16,
}

impl<FB: FrameBuffer> NomParser<FB> {
    pub fn new(fb: Arc<FB>) -> Self {
        Self {
            fb,
            connection_x_offset: 0,
            connection_y_offset: 0,
        }
    }
}

#[derive(Debug)]
pub enum Request {
    Help,
    Size,
    GetPixel { x: u16, y: u16 },
    SetPixel { x: u16, y: u16, rgba: u32 },
}

impl<FB: FrameBuffer> Parser for NomParser<FB> {
    #[inline(always)]
    fn parse(&mut self, buffer: &[u8], response: &mut Vec<u8>) -> usize {
        let mut remaining = buffer;
        loop {
            match parse_request(remaining).finish() {
                Ok((r, request)) => {
                    remaining = r;
                    self.handle_request(request, response);
                }
                Err(_err) => {
                    // dbg!(&err);

                    return buffer.len();
                    // let wrong_input = err.input;

                    // buffer = &buffer[wrong_input.len()..];
                    // buffer.take(..wrong_input.len());

                    //panic!("Don't know what to do with this error: {err}"
                }
            };
        }
    }

    #[inline(always)]
    fn parser_lookahead(&self) -> usize {
        0
    }
}

impl<FB: FrameBuffer> NomParser<FB> {
    #[inline(always)]
    fn handle_request(&mut self, request: Request, response: &mut Vec<u8>) {
        // dbg!(&request);

        match request {
            Request::Help => {
                response.extend_from_slice(HELP_TEXT);
                // TODO: Count HELPs and reject spammers
            }
            Request::Size => {
                response.extend_from_slice(
                    format!("SIZE {} {}\n", self.fb.get_width(), self.fb.get_height()).as_bytes(),
                );
            }
            Request::GetPixel { x, y } => {
                if let Some(rgb) = self.fb.get(x as usize, y as usize) {
                    response.extend_from_slice(
                        format!(
                            "PX {} {} {:06x}\n",
                            // We don't want to return the actual (absolute) coordinates, the client should also get the result offseted
                            x - self.connection_x_offset,
                            y - self.connection_y_offset,
                            rgb.to_be() >> 8
                        )
                        .as_bytes(),
                    );
                }
            }
            Request::SetPixel { x, y, rgba } => {
                self.fb.set(x as usize, y as usize, rgba & 0x00ff_ffff);
            }
        }
    }
}

fn parse_request(i: &[u8]) -> IResult<&[u8], Request> {
    // Trying to sort descending by number of occurrences for performance reasons
    terminated(
        alt((parse_get_or_set_pixel, parse_size, parse_help)),
        char('\n'),
    )(i)
}

fn parse_help(i: &[u8]) -> IResult<&[u8], Request> {
    map(tag("HELP"), |_| Request::Help)(i)
}

fn parse_size(i: &[u8]) -> IResult<&[u8], Request> {
    map(tag("SIZE"), |_| Request::Size)(i)
}

fn parse_get_or_set_pixel(i: &[u8]) -> IResult<&[u8], Request> {
    let (i, (x, y)) = preceded(
        tag("PX "),
        separated_pair(
            nom::character::complete::u16,
            char(' '),
            nom::character::complete::u16,
        ),
    )(i)?;

    // Read request, as there are no following bytes
    if i.first() == Some(&b'\n') {
        return Ok((i, Request::GetPixel { x, y }));
    }

    // As there are bytes left, this needs to be a SetPixel request
    let (i, rgba) = preceded(char(' '), ascii_hex_u32)(i)?;

    Ok((i, Request::SetPixel { x, y, rgba }))
}

fn ascii_hex_u32(i: &[u8]) -> IResult<&[u8], u32> {
    map_res(
        take_while_m_n(6, 6, |c: u8| c.is_ascii_hexdigit()),
        |hex: &[u8]| {
            // SAFETY: This can only be called on hexdigits!
            let hex_str = unsafe { str::from_utf8_unchecked(hex) };
            Ok::<u32, ParseIntError>(u32::from_be(u32::from_str_radix(hex_str, 16)? << 8))
        },
    )(i)
}
