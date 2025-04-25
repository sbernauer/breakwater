use core::slice;
use std::{cell::UnsafeCell, pin::Pin};

use color_eyre::eyre::{self, Context, bail};
use shared_memory::{Shmem, ShmemConf, ShmemError};
use tracing::{debug, info, instrument};

use super::FrameBuffer;
use crate::framebuffer::FB_BYTES_PER_PIXEL;

// Width and height, both of type u16.
const HEADER_SIZE: usize = 2 * std::mem::size_of::<u16>();

unsafe impl Send for SharedMemoryFrameBuffer {}
unsafe impl Sync for SharedMemoryFrameBuffer {}

pub struct SharedMemoryFrameBuffer {
    width: usize,
    height: usize,

    bytes: usize,

    // This owns the memory, but is never accessed
    #[allow(unused)]
    memory: MemoryType,

    // This is a reference to the owned memory
    // Safety: valid as long as memory won`t change/move/...
    buffer: Pin<&'static [UnsafeCell<u8>]>,
}

// This owns the memory, but is never accessed
#[allow(unused)]
enum MemoryType {
    Shared(Shmem),
    Local(Pin<Box<[UnsafeCell<u8>]>>),
}

impl SharedMemoryFrameBuffer {
    #[instrument]
    pub fn new(
        width: usize,
        height: usize,
        shared_memory_name: Option<&str>,
    ) -> eyre::Result<Self> {
        match shared_memory_name {
            Some(shared_memory_name) => {
                Self::new_from_shared_memory(width, height, shared_memory_name)
            }
            None => Self::new_with_local_memory(width, height),
        }
    }

    #[instrument(skip_all)]
    fn new_with_local_memory(width: usize, height: usize) -> eyre::Result<Self> {
        let pixels = width * height;
        let bytes = pixels * FB_BYTES_PER_PIXEL;

        debug!("Using plain (non shared memory) framebuffer");

        let memory: Pin<Box<[UnsafeCell<u8>]>> = Pin::new(
            (0..(bytes))
                .map(|_| UnsafeCell::new(0u8))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let buffer = unsafe {
            std::mem::transmute::<Pin<&[UnsafeCell<u8>]>, Pin<&'static [UnsafeCell<u8>]>>(
                memory.as_ref(),
            )
        };

        Ok(Self {
            width,
            height,
            bytes,
            memory: MemoryType::Local(memory),
            buffer,
        })
    }

    #[instrument(skip_all)]
    fn new_from_shared_memory(
        width: usize,
        height: usize,
        shared_memory_name: &str,
    ) -> eyre::Result<Self> {
        let pixels = width * height;
        let framebuffer_bytes = pixels * FB_BYTES_PER_PIXEL;
        let target_size = HEADER_SIZE + framebuffer_bytes;

        let mut shared_memory = match ShmemConf::new()
            .os_id(shared_memory_name)
            .size(target_size)
            .create()
        {
            Ok(shared_memory) => shared_memory,
            Err(ShmemError::LinkExists | ShmemError::MappingIdExists) => ShmemConf::new()
                .os_id(shared_memory_name)
                .open()
                .with_context(|| {
                    format!("failed to open existing shared memory \"{shared_memory_name}\"")
                })?,
            Err(err) => Err(err).with_context(|| {
                format!("failed to create shared memory \"{shared_memory_name}\"")
            })?,
        };

        // In case we crate the shared memory we are the owner. In that case `shared_memory` will
        // delete the shared memory on `drop`. As we want to persist the framebuffer across
        // restarts, we set the owner to false.
        shared_memory.set_owner(false);

        let actual_size = shared_memory.len();
        if actual_size != target_size {
            bail!(
                "The shared memory had the wrong size! Expected {target_size} bytes, \
                        but it has {actual_size} bytes."
            );
        }

        info!(
            actual_size,
            name = shared_memory_name,
            target_size,
            "Shared memory loaded"
        );
        let size_ptr = shared_memory.as_ptr() as *mut u16;
        unsafe {
            *size_ptr = width.try_into().context("Framebuffer width too high")?;
            *size_ptr.add(1) = height.try_into().context("Framebuffer height too high")?;
        }

        // We need to skip the 4 header bytes
        let framebuffer_base_ptr = unsafe { shared_memory.as_ptr().add(HEADER_SIZE) };
        let buffer = unsafe {
            let data = framebuffer_base_ptr as *const UnsafeCell<u8>;
            let slice = Pin::new(slice::from_raw_parts(data, framebuffer_bytes));
            std::mem::transmute::<Pin<&[UnsafeCell<u8>]>, Pin<&'static [UnsafeCell<u8>]>>(slice)
        };

        Ok(Self {
            width,
            height,
            bytes: framebuffer_bytes,
            memory: MemoryType::Shared(shared_memory),
            buffer,
        })
    }
}

impl FrameBuffer for SharedMemoryFrameBuffer {
    #[inline(always)]
    fn get_width(&self) -> usize {
        self.width
    }

    #[inline(always)]
    fn get_height(&self) -> usize {
        self.height
    }

    #[inline(always)]
    unsafe fn get_unchecked(&self, x: usize, y: usize) -> u32 {
        debug_assert!(x < self.width);
        debug_assert!(y < self.height);

        let offset = (x + y * self.width) * FB_BYTES_PER_PIXEL;

        let base_ptr = self.buffer.as_ptr() as *const u8;
        let pixel_ptr = unsafe { base_ptr.add(offset) } as *const u32;

        // The buffer coming from the shared memory might be unaligned!
        unsafe { pixel_ptr.read_unaligned() }
    }

    #[inline(always)]
    fn set(&self, x: usize, y: usize, rgba: u32) {
        // See 'SimpleFrameBuffer::set' for performance consideration
        if x < self.width && y < self.height {
            let offset = (x + y * self.width) * FB_BYTES_PER_PIXEL;
            let pixel_ptr = unsafe { self.buffer.get_unchecked(offset).get() } as *mut u32;

            // The buffer coming from the shared memory might be unaligned!
            unsafe { pixel_ptr.write_unaligned(rgba) }
        }
    }

    #[inline(always)]
    fn set_multi_from_start_index(&self, starting_index: usize, pixels: &[u8]) -> usize {
        let num_pixels = pixels.len() / 4;

        if starting_index + num_pixels > self.get_size() {
            debug!(
                starting_index,
                num_pixels,
                buffer_bytes = self.bytes,
                "Ignoring invalid set_multi call, which would exceed the screen",
            );
            // We did not move
            return 0;
        }

        let starting_ptr = unsafe { self.buffer.get_unchecked(starting_index) }.get();
        let target_slice = unsafe { slice::from_raw_parts_mut(starting_ptr, pixels.len()) };
        target_slice.copy_from_slice(pixels);

        num_pixels
    }

    #[inline(always)]
    fn as_bytes(&self) -> &[u8] {
        let base_ptr = self.buffer.as_ptr() as *const u8;
        unsafe { slice::from_raw_parts(base_ptr as *mut u8, self.bytes) }
    }
}
