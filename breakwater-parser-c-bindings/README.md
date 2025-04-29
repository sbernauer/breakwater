# breakwater-parser-c-bindings

This bindings allows you to call the breakwater-parsed fom C code (or all other languages that offer FFI with C functions).

The bindings can be build using `cargo build --release -p breakwater-parser-c-bindings` and will be placed at `target/release/libbreakwater_parser_c_bindings.so`.

For the function signatures and docs please have a look at the Rust docs in `breakwater-parser-c-bindings/src/lib.rs`.

For example usage please have a look at `breakwater-parser-c-bindings/test-from-c/test.c`.
