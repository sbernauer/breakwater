# How to build a custom overlay for breakwater

## Create a new project that compiles to a dynamic library

```bash
cargo init --lib my-awesome-overlay
```

`Cargo.toml`
```toml
[package]
name = "my-awesome-overlay"
version = "0.1.0"
edition = "2024"

[dependencies]
breakwater-egui-overlay = { git = "https://github.com/sbernauer/breakwater" }

[lib]
crate-type = ["dylib"]
```

## Add Boilerplate

### Export a function that exposes some version information for version checks

```rust
#[unsafe(no_mangle)]
pub extern "C" fn versions() -> breakwater_egui_overlay::Versions {
    breakwater_egui_overlay::VERSIONS
}
```

### (Optional) Create function that breakwater will call for setup and teardown

```rust
static NEW_FN: breakwater_egui_overlay::New = ui_new as _;
extern "C" fn ui_new(data: *mut std::ffi::c_void) {
    // data points to custum your data

    println!("Hello breakwater");
}
static DROP_FN: breakwater_egui_overlay::Drop = drop as _;
extern "C" fn drop(data: *mut std::ffi::c_void) {
    // data points to custum your data
    
    println!("Goodbye breakwater");
}
```

### Put everything together

```rust
pub const OVERLAY: DynamicOverlay = DynamicOverlay {
    // you may use this pointer for custom data.
    // breakwater will pass this pointer to all calls.
    data: std::ptr::null_mut(),

    // add your setup function here other wise `std::ptr::null()`
    new: &raw const NEW_FN,

    // add your draw_ui function here. THIS IS REQUIRED!
    draw_ui: draw_ui as _,
    
    // add your teradown function here other wise `std::ptr::null()`
    drop: &raw const DROP_FN,
};

// expose your overlay
#[unsafe(no_mangle)]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn new() -> breakwater_egui_overlay::DynamicOverlay {
    OVERLAY
}
```

## Implement your `draw_ui` function

```rust
#[allow(improper_ctypes_definitions)]
extern "C" fn draw_ui(
    _: *mut std::ffi::c_void,
    viewport_idx: u32,
    ctx: &egui::Context,
    _advertised_endpoints: &[String],
    connections: u32,
    ips_v6: u32,
    ips_v4: u32,
    bytes_per_s: u64,
) { 
  // your fancy egui widgets go here
}
```

## Compile and use

Compile your overlay

```bash
cargo build
cargo build --release
# find your overlay in ./target/{debug,release}/my-awesome-overlay.so
```

Use it with breakwater

```bash
breakwater --ui path/to/your/overlay.so
```

**Note**: It is important that you match the compiler version and configuration between breakwater and your overlay.
That includes that you only use the `--release` version of breakwater with the `--release` version of your overlay.

## Examples

- [38c3 overlay](https://github.com/bits0rcerer/breakwater-38c3-overlay)
  - you can use this as a template and customize it to your needs

You can find implementation details [here](../breakwater-egui-overlay/src/lib.rs)
