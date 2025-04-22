use core::slice;
use std::{
    alloc::{self, Layout},
    cell::UnsafeCell,
    ptr::NonNull,
};

use color_eyre::eyre::{self, Context, ContextCompat, bail};
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
    buffer: NonNull<UnsafeCell<[u8]>>,

    deallocate_buffer_on_drop: bool,

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
        let bytes = pixels * FB_BYTES_PER_PIXEL;

        let Some(shared_memory_name) = shared_memory_name else {
            debug!("Using plain (non shared memory) framebuffer");

            let layout = Layout::array::<u8>(bytes)
                .context("Invalid memory layout for framebuffer buffer")?;
            let ptr = unsafe { alloc::alloc(layout) };
            if ptr.is_null() {
                bail!("Failed to allocate framebuffer memory (returned pointer was null)");
            }
            let slice_ptr: *mut [u8] = std::ptr::slice_from_raw_parts_mut(ptr, bytes);
            let cell_ptr = slice_ptr as *mut UnsafeCell<[u8]>;
            let buffer =
                NonNull::new(cell_ptr).context("failed to create non-null framebuffer buffer")?;

            return Ok(Self {
                width,
                height,
                bytes,
                buffer,
                shared_memory: None,
                deallocate_buffer_on_drop: true,
            });
        };

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
                    format!("failed to open existing shared memory \"{shared_memory_name}\"")
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
        let fb_base_ptr = unsafe { shared_memory.as_ptr().add(4) };
        let slice_ptr: *mut [u8] = std::ptr::slice_from_raw_parts_mut(fb_base_ptr, bytes);
        let cell_ptr = slice_ptr as *mut UnsafeCell<[u8]>;
        let buffer = NonNull::new(cell_ptr)
            .context("failed to create non-null framebuffer buffer from shared memory")?;

        Ok(Self {
            width,
            height,
            bytes,
            buffer,
            shared_memory: Some(shared_memory),
            // When the `Shmem` get's dropped, it automatically frees the underlying memory
            deallocate_buffer_on_drop: false,
        })
    }
}

impl Drop for SharedMemoryFrameBuffer {
    fn drop(&mut self) {
        if !self.deallocate_buffer_on_drop {
            return;
        }

        let ptr = self.buffer.as_ptr() as *mut u8;
        // We can not use "normal" error handling here, so we expect() instead
        let layout =
            Layout::array::<u8>(self.bytes).expect("Invalid memory layout for framebuffer buffer");
        unsafe {
            alloc::dealloc(ptr, layout);
        }
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
            let base_ptr = unsafe { self.buffer.as_ref().get() } as *mut u8;
            let pixel_ptr = unsafe { base_ptr.add(offset) } as *mut u32;

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

        let base_ptr = unsafe { self.buffer.as_ref().get() } as *mut u8;
        let target_slice = unsafe { slice::from_raw_parts_mut(base_ptr, pixels.len()) };
        target_slice.copy_from_slice(pixels);

        num_pixels
    }

    #[inline(always)]
    fn as_bytes(&self) -> &[u8] {
        let base_ptr = self.buffer.as_ptr() as *const u8;
        unsafe { slice::from_raw_parts(base_ptr as *mut u8, self.bytes) }
    }
}
