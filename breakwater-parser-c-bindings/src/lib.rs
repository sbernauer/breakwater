use core::str;
use std::{
    ffi::c_int,
    slice,
    sync::{Arc, Mutex, OnceLock},
};

use breakwater_parser::{OriginalParser, Parser, SimpleFrameBuffer};
use libc::size_t;

static ORIGINAL_PARSER: OnceLock<Mutex<OriginalParser<SimpleFrameBuffer>>> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn breakwater_init_original_parser(width: c_int, height: c_int) {
    ORIGINAL_PARSER.get_or_init(|| {
        // let fb = Arc::new(NoopFrameBuffer::new(
        //     width.try_into().unwrap(),
        //     height.try_into().unwrap(),
        // ));
        let fb = Arc::new(SimpleFrameBuffer::new(
            width.try_into().unwrap(),
            height.try_into().unwrap(),
        ));

        Mutex::new(OriginalParser::new(fb))
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn breakwater_original_parser_parser_lookahead() -> size_t {
    let parser = ORIGINAL_PARSER
        .get()
        .expect("Call breakwater_init_original_parser first!");

    parser
        .lock()
        .unwrap()
        .parser_lookahead()
        .try_into()
        .unwrap()
}

#[unsafe(no_mangle)]
pub extern "C" fn breakwater_original_parser_parse(buffer: *mut u8, buffer_len: size_t) -> size_t {
    let parser = ORIGINAL_PARSER
        .get()
        .expect("Call breakwater_init_original_parser first!");

    let buffer = unsafe { slice::from_raw_parts(buffer, buffer_len.try_into().unwrap()) };

    // FIXME: Somehow return the response to the C side
    let mut response = Vec::new();
    let parsed = { parser.lock().unwrap().parse(buffer, &mut response) };
    dbg!(str::from_utf8(&response).unwrap());

    parsed.try_into().unwrap()
}
