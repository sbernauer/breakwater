use std::sync::Arc;

use breakwater_core::{framebuffer::FrameBuffer, HELP_TEXT};
use snafu::ResultExt;
use tokio::io::AsyncWriteExt;

use crate::{
    original::{
        parse_pixel_coordinates, simd_unhex, HELP_PATTERN, OFFSET_PATTERN, PX_PATTERN, SIZE_PATTERN,
    },
    Parser, ParserError,
};

const PARSER_LOOKAHEAD: usize = "PX 1234 1234 rrggbbaa\n".len(); // Longest possible command

pub struct RefactoredParser {
    connection_x_offset: usize,
    connection_y_offset: usize,
    fb: Arc<FrameBuffer>,
}

impl RefactoredParser {
    pub fn new(fb: Arc<FrameBuffer>) -> Self {
        Self {
            connection_x_offset: 0,
            connection_y_offset: 0,
            fb,
        }
    }

    #[inline(always)]
    async fn handle_pixel(
        &self,
        buffer: &[u8],
        mut idx: usize,
        stream: &mut (impl AsyncWriteExt + Send + Unpin),
    ) -> Result<(usize, usize), ParserError> {
        let previous = idx;
        idx += 3;

        let (mut x, mut y, present) = parse_pixel_coordinates(buffer.as_ptr(), &mut idx);

        if present {
            x += self.connection_x_offset;
            y += self.connection_y_offset;

            // Separator between coordinates and color
            if unsafe { *buffer.get_unchecked(idx) } == b' ' {
                idx += 1;

                // TODO: Determine what clients use more: RGB, RGBA or gg variant.
                // If RGBA is used more often move the RGB code below the RGBA code

                // Must be followed by 6 bytes RGB and newline or ...
                if unsafe { *buffer.get_unchecked(idx + 6) } == b'\n' {
                    idx += 7;
                    self.handle_rgb(idx, buffer, x, y);
                    Ok((idx, idx))
                }
                // ... or must be followed by 8 bytes RGBA and newline
                else if unsafe { *buffer.get_unchecked(idx + 8) } == b'\n' {
                    idx += 9;
                    self.handle_rgba(idx, buffer, x, y);
                    Ok((idx, idx))
                }
                // ... for the efficient/lazy clients
                else if unsafe { *buffer.get_unchecked(idx + 2) } == b'\n' {
                    idx += 3;
                    self.handle_gray(idx, buffer, x, y);
                    Ok((idx, idx))
                } else {
                    Ok((idx, previous))
                }
            }
            // End of command to read Pixel value
            else if unsafe { *buffer.get_unchecked(idx) } == b'\n' {
                idx += 1;
                self.handle_get_pixel(stream, x, y).await?;
                Ok((idx, idx))
            } else {
                Ok((idx, previous))
            }
        } else {
            Ok((idx, previous))
        }
    }

    #[inline(always)]
    fn handle_offset(&mut self, idx: &mut usize, buffer: &[u8]) {
        let (x, y, present) = parse_pixel_coordinates(buffer.as_ptr(), idx);

        // End of command to set offset
        if present && unsafe { *buffer.get_unchecked(*idx) } == b'\n' {
            self.connection_x_offset = x;
            self.connection_y_offset = y;
        }
    }

    #[inline(always)]
    async fn handle_size(
        &self,
        stream: &mut (impl AsyncWriteExt + Send + Unpin),
    ) -> Result<(), ParserError> {
        stream
            .write_all(
                format!("SIZE {} {}\n", self.fb.get_width(), self.fb.get_height()).as_bytes(),
            )
            .await
            .context(crate::WriteToTcpSocketSnafu)?;
        Ok(())
    }

    #[inline(always)]
    async fn handle_help(
        &self,
        stream: &mut (impl AsyncWriteExt + Send + Unpin),
    ) -> Result<(), ParserError> {
        stream
            .write_all(HELP_TEXT)
            .await
            .context(crate::WriteToTcpSocketSnafu)?;
        Ok(())
    }

    #[inline(always)]
    fn handle_rgb(&self, idx: usize, buffer: &[u8], x: usize, y: usize) {
        let rgba: u32 = simd_unhex(unsafe { buffer.as_ptr().add(idx - 7) });

        self.fb.set(x, y, rgba & 0x00ff_ffff);
    }

    #[cfg(not(feature = "alpha"))]
    #[inline(always)]
    fn handle_rgba(&self, idx: usize, buffer: &[u8], x: usize, y: usize) {
        let rgba: u32 = simd_unhex(unsafe { buffer.as_ptr().add(idx - 9) });

        self.fb.set(x, y, rgba & 0x00ff_ffff);
    }

    #[cfg(feature = "alpha")]
    #[inline(always)]
    fn handle_rgba(&self, idx: usize, buffer: &[u8], x: usize, y: usize) {
        let rgba: u32 = simd_unhex(unsafe { buffer.as_ptr().add(idx - 9) });

        let alpha = (rgba >> 24) & 0xff;

        if alpha == 0 || x >= self.fb.get_width() || y >= self.fb.get_height() {
            return;
        }

        let alpha_comp = 0xff - alpha;
        let current = self.fb.get_unchecked(x, y);
        let r = (rgba >> 16) & 0xff;
        let g = (rgba >> 8) & 0xff;
        let b = rgba & 0xff;

        let r: u32 = (((current >> 24) & 0xff) * alpha_comp + r * alpha) / 0xff;
        let g: u32 = (((current >> 16) & 0xff) * alpha_comp + g * alpha) / 0xff;
        let b: u32 = (((current >> 8) & 0xff) * alpha_comp + b * alpha) / 0xff;

        self.fb.set(x, y, r << 16 | g << 8 | b);
    }

    #[inline(always)]
    fn handle_gray(&self, idx: usize, buffer: &[u8], x: usize, y: usize) {
        // FIXME: Read that two bytes directly instead of going through the whole SIMD vector setup.
        // Or - as an alternative - still do the SIMD part but only load two bytes.
        let base: u32 = simd_unhex(unsafe { buffer.as_ptr().add(idx - 3) }) & 0xff;

        let rgba: u32 = base << 16 | base << 8 | base;

        self.fb.set(x, y, rgba);
    }

    #[inline(always)]
    async fn handle_get_pixel(
        &self,
        stream: &mut (impl AsyncWriteExt + Send + Unpin),
        x: usize,
        y: usize,
    ) -> Result<(), ParserError> {
        if let Some(rgb) = self.fb.get(x, y) {
            stream
                .write_all(
                    format!(
                        "PX {} {} {:06x}\n",
                        // We don't want to return the actual (absolute) coordinates, the client should also get the result offseted
                        x - self.connection_x_offset,
                        y - self.connection_y_offset,
                        rgb.to_be() >> 8
                    )
                    .as_bytes(),
                )
                .await
                .context(crate::WriteToTcpSocketSnafu)?;
        }
        Ok(())
    }
}

impl Parser for RefactoredParser {
    async fn parse(
        &mut self,
        buffer: &[u8],
        mut stream: impl AsyncWriteExt + Send + Unpin,
    ) -> Result<usize, ParserError> {
        let mut last_byte_parsed = 0;

        let mut i = 0; // We can't use a for loop here because Rust don't lets use skip characters by incrementing i
        let loop_end = buffer.len().saturating_sub(PARSER_LOOKAHEAD); // Let's extract the .len() call and the subtraction into it's own variable so we only compute it once

        while i < loop_end {
            let current_command =
                unsafe { (buffer.as_ptr().add(i) as *const u64).read_unaligned() };
            if current_command & 0x00ff_ffff == PX_PATTERN {
                (i, last_byte_parsed) = self.handle_pixel(buffer, i, &mut stream).await?;
            } else if current_command & 0x00ff_ffff_ffff_ffff == OFFSET_PATTERN {
                i += 7;
                self.handle_offset(&mut i, buffer);
                last_byte_parsed = i;
            } else if current_command & 0xffff_ffff == SIZE_PATTERN {
                i += 4;
                last_byte_parsed = i;
                self.handle_size(&mut stream).await?;
            } else if current_command & 0xffff_ffff == HELP_PATTERN {
                i += 4;
                last_byte_parsed = i;
                self.handle_help(&mut stream).await?;
            } else {
                i += 1;
            }
        }

        Ok(last_byte_parsed.wrapping_sub(1))
    }

    fn parser_lookahead(&self) -> usize {
        PARSER_LOOKAHEAD
    }
}
