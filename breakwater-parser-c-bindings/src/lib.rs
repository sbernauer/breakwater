use std::{
    ffi::{CStr, c_char, c_int},
    slice,
    sync::{Arc, Mutex, OnceLock},
};

use breakwater_parser::{OriginalParser, Parser, SharedMemoryFrameBuffer};
use libc::size_t;

static ORIGINAL_PARSER: OnceLock<Mutex<OriginalParser<SharedMemoryFrameBuffer>>> = OnceLock::new();

/// Initialize the original parser. It creates a framebuffer of the specified size, internally
/// backed by shared memory.
///
/// Function is thread safe (I guess).
///
/// # Safety
///
/// Arguments:
///
/// 1 `width` (`int`): The width of the canvas in pixels
/// 2 `height`(`int`): The height of the canvas in pixels
/// 3. `shared_memory_name_ptr` (`char []`): The name of the shared memory region to create/use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn breakwater_init_original_parser(
    width: c_int,
    height: c_int,
    shared_memory_name_ptr: *const c_char,
) {
    let shared_memory_name = unsafe { CStr::from_ptr(shared_memory_name_ptr) }
        .to_str()
        .expect("Invalid shared_memory_name String!");
    ORIGINAL_PARSER.get_or_init(|| {
        let fb = Arc::new(
            SharedMemoryFrameBuffer::new(
                width.try_into().unwrap(),
                height.try_into().unwrap(),
                Some(shared_memory_name),
            )
            .expect("Failed to create shared-memory framebuffer"),
        );

        Mutex::new(OriginalParser::new(fb))
    });
}

/// Return the parsed lookahead. This number of bytes needs to be readable by the program without
/// segfaulting.
///
/// Function is thread safe (I guess).
///
/// Function has no arguments.
#[unsafe(no_mangle)]
pub extern "C" fn breakwater_original_parser_parser_lookahead() -> size_t {
    let parser = ORIGINAL_PARSER
        .get()
        .expect("Call breakwater_init_original_parser first!");

    parser.lock().unwrap().parser_lookahead()
}

/// Parse the given user input.
///
/// Function is thread safe (I guess).
///
/// # Safety
///
/// Arguments:
///
/// 1. `buffer` (`const char*`): The bytes the user send.
/// 2. `buffer_len` (`size_t`): The number of bytes to parse. Please remember the
///    [`breakwater_original_parser_parser_lookahead`] parser lookahead!
/// 3. `out_response_ptr` (`unsigned char**`): This pointer will be changed to point to the bytes
///    forming the response to the client. **It's your responsibility to free the passed memory!**
/// 4. `out_response_len` (`size_t*`): This number will be changed to the number of bytes the output
///    for the client has.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn breakwater_original_parser_parse(
    buffer: *mut u8,
    buffer_len: size_t,
    out_response_ptr: *mut *mut u8,
    out_response_len: *mut size_t,
) -> size_t {
    let parser = ORIGINAL_PARSER
        .get()
        .expect("Call breakwater_init_original_parser first!");

    let buffer = unsafe { slice::from_raw_parts(buffer, buffer_len) };

    // We don't know how many bytes to allocate upfront. Most clients will probably only send write-
    // traffic and not read much.
    let mut response = Vec::new();
    let parsed = { parser.lock().unwrap().parse(buffer, &mut response) };

    // Leak the response Vec into raw memory so C can own it
    let response_ptr = response.as_mut_ptr();
    let response_len = response.len();

    if !out_response_ptr.is_null() {
        unsafe {
            *out_response_ptr = response_ptr;
        }
    }
    if !out_response_len.is_null() {
        unsafe { *out_response_len = size_t::from(response_len) };
    }

    // Don't free the Vec — the C caller will have to free it later!
    std::mem::forget(response);

    parsed
}
