use std::alloc::{self, LayoutError};

use log::warn;
use memadvise::{Advice, MemAdviseError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to create memory layout")]
    CreateMemoryLayout(#[from] LayoutError),

    #[error("allocation failed (alloc::alloc returned null ptr) for layout {layout:?}")]
    AllocationFailed { layout: alloc::Layout },
}

pub struct ConnectionBuffer {
    ptr: *mut u8,
    layout: alloc::Layout,
}

/// Safety:
/// - `ConnectionBuffer` has exclusive ownership of the memory behind `ptr`
/// - safe access through `ConnectionBuffer::as_slice_mut()`
unsafe impl Send for ConnectionBuffer {}

/// Allocates a memory slice with the specified size, which can be used for client connections.
///
/// It takes care of de-allocating the memory slice on [`Drop`].
/// It also `memadvise`s the memory slice, so that the Kernel is aware that we are going to
/// sequentially read it.
impl ConnectionBuffer {
    pub fn new(buffer_size: usize) -> Result<Self, Error> {
        let page_size = page_size::get();
        let layout = alloc::Layout::from_size_align(buffer_size, page_size)?;

        let ptr = unsafe { alloc::alloc(layout) };

        if ptr.is_null() {
            return Err(Error::AllocationFailed { layout });
        }

        if let Err(err) = memadvise::advise(ptr as _, layout.size(), Advice::Sequential) {
            // [`MemAdviseError`] does not implement Debug...
            let err = match err {
                MemAdviseError::NullAddress => "NullAddress",
                MemAdviseError::InvalidLength => "InvalidLength",
                MemAdviseError::UnalignedAddress => "UnalignedAddress",
                MemAdviseError::InvalidRange => "InvalidRange",
            };
            warn!(
                "Failed to memadvise sequential read access for buffer to kernel. This should not effect any client connections, but might having some minor performance degration: {err}"
            );
        }

        Ok(Self { ptr, layout })
    }

    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }
}

impl Drop for ConnectionBuffer {
    fn drop(&mut self) {
        unsafe {
            alloc::dealloc(self.ptr, self.layout);
        }
    }
}
