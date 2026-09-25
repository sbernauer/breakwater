// The new trait solver used by current nightly needs a deeper recursion limit to prove `Send`/`Sync`
// for deeply nested wgpu types. E.g. `egui_wgpu::CallbackTrait` requires `CanvasUpload: Sync`,
// which walks all the way through the internals of `wgpu::Texture`. With the default limit of 128
// we get the future-incompatible `recursion_depth_exceeding_limit` warning, which will become a
// hard error. wgpu itself raises the limit to 256 for the same reason.
//
// See https://github.com/rust-lang/rust/issues/159228 and https://github.com/gfx-rs/wgpu/issues/9608
//
// TODO: Remove once we use a wgpu release containing https://github.com/gfx-rs/wgpu/pull/9953,
// which implements `Send`/`Sync` manually, so downstream crates don't need this anymore.
#![recursion_limit = "256"]

pub mod cli_args;
pub mod connection_buffer;
#[cfg(feature = "prometheus")]
pub mod prometheus_exporter;
pub mod server;
pub mod sinks;
pub mod statistics;

#[cfg(test)]
pub mod test_helpers;
#[cfg(test)]
pub mod tests;
