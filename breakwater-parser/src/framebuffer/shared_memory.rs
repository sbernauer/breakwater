use core::slice;

use color_eyre::eyre::{self, Context, bail};
use shared_memory::{Shmem, ShmemConf, ShmemError};
use tracing::{info, instrument};

use super::FrameBuffer;
use crate::framebuffer::FB_BYTES_PER_PIXEL;

// Width and height, both of type u16.
const HEADER_SIZE: usize = 2 * std::mem::size_of::<u16>();

unsafe impl Send for SharedMemoryFrameBuffer {}
unsafe impl Sync for SharedMemoryFrameBuffer {}

pub struct SharedMemoryFrameBuffer {
    width: usize,
    height: usize,
    buffer: Box<[u8]>,

    /// We need to keep the shared memory (so not drop it), otherwise we get a segfault.
    #[allow(unused)]
    shared_memory: Option<Shmem>,
}

impl SharedMemoryFrameBuffer {
    #[instrument]
    pub fn new(
        width: usize,
        height: usize,
        shared_memory_name: Option<&str>,
    ) -> eyre::Result<Self> {
        let pixels = width * height;

        match shared_memory_name {
            Some(shared_memory_name) => {
                let target_size = HEADER_SIZE + pixels * FB_BYTES_PER_PIXEL;

                let shared_memory = match ShmemConf::new()
                    .os_id(shared_memory_name)
                    .size(target_size)
                    .create()
                {
                    Ok(shared_memory) => shared_memory,
                    Err(ShmemError::LinkExists | ShmemError::MappingIdExists) => ShmemConf::new()
                        .os_id(shared_memory_name)
                        .open()
                        .with_context(|| {
                            format!(
                                "failed to open existing shared memory \"{shared_memory_name}\""
                            )
                        })?,
                    Err(err) => Err(err).with_context(|| {
                        format!("failed to create shared memory \"{shared_memory_name}\"")
                    })?,
                };

                let actual_size = shared_memory.len();
                if actual_size != target_size {
                    bail!(
                        "The shared memory had the wrong size! Expected {target_size} bytes, \
                        but it has {actual_size} bytes."
                    );
                }

                info!(
                    actual_size,
                    target_size, "Shared memory \"shared_memory_name\" loaded"
                );
                let size_ptr = shared_memory.as_ptr() as *mut u16;
                unsafe {
                    *size_ptr = width.try_into().context("Framebuffer width too high")?;
                    *size_ptr.add(1) = height.try_into().context("Framebuffer height too high")?;
                }

                let buffer = unsafe {
                    Box::from_raw(slice::from_raw_parts_mut(
                        shared_memory.as_ptr().add(2),
                        pixels * FB_BYTES_PER_PIXEL,
                    ))
                };

                Ok(Self {
                    width,
                    height,
                    buffer,
                    shared_memory: Some(shared_memory),
                })
            }
            None => {
                let mut buffer = Vec::with_capacity(pixels * FB_BYTES_PER_PIXEL);
                buffer.resize_with(pixels * FB_BYTES_PER_PIXEL, || 0);
                let buffer = buffer.into_boxed_slice();

                Ok(Self {
                    width,
                    height,
                    buffer,
                    shared_memory: None,
                })
            }
        }
    }
}

impl FrameBuffer for SharedMemoryFrameBuffer {
    fn get_width(&self) -> usize {
        self.width
    }

    fn get_height(&self) -> usize {
        self.height
    }

    unsafe fn get_unchecked(&self, x: usize, y: usize) -> u32 {
        unsafe {
            *(self
                .buffer
                .as_ptr()
                .add((x + y * self.width) * FB_BYTES_PER_PIXEL) as *const u32)
        }
    }

    fn set(&self, x: usize, y: usize, rgba: u32) {
        // See 'SimpleFrameBuffer::set' for performance consideration
        if x < self.width && y < self.height {
            unsafe {
                let ptr = self
                    .buffer
                    .as_ptr()
                    .add((x + y * self.width) * FB_BYTES_PER_PIXEL)
                    as *mut u32;
                // The buffer coming from the shared memory might be unaligned!
                ptr.write_unaligned(rgba)
            }
        }
    }

    fn set_multi_from_start_index(&self, starting_index: usize, pixels: &[u8]) -> usize {
        let num_pixels = pixels.len() / 4;

        if starting_index + num_pixels > self.width * self.height {
            dbg!(
                "Ignoring invalid set_multi call, which would exceed the screen",
                starting_index,
                num_pixels,
                self.width * self.height,
            );
            // We did not move
            return 0;
        }

        let starting_ptr = unsafe { self.buffer.as_ptr().add(starting_index) };
        let target_slice =
            unsafe { slice::from_raw_parts_mut(starting_ptr as *mut u8, pixels.len()) };
        target_slice.copy_from_slice(pixels);

        num_pixels
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buffer
    }
}
