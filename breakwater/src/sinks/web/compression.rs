//! Compressing the frames the web sink sends to its viewers.
//!
//! The frames always go out as ordinary zlib streams, because [`DecompressionStream`] is the only
//! decompressor browsers hand us for free and it only speaks `gzip`, `deflate` and `deflate-raw`.
//! *Which* encoder produces those streams is a build-time choice, as they differ a lot in how much
//! CPU they spend for a given compression - see `README.md` next to this file for measurements.
//!
//! [`DecompressionStream`]: https://developer.mozilla.org/en-US/docs/Web/API/DecompressionStream

use color_eyre::eyre;

trait CompressionBackend: Copy {
    const MAX_LEVEL: u32;

    fn new(level: u32) -> eyre::Result<Self>;

    fn compress(self, data: &[u8]) -> eyre::Result<Vec<u8>>;
}

/// The default: zlib-rs, the fastest of the zlib-compatible encoders and pure Rust, so the web
/// sink stays buildable without a C toolchain.
#[cfg(not(feature = "web-libdeflate"))]
mod backend {
    use std::io::Write;

    use color_eyre::eyre::{self, Context};
    use flate2::{Compression, write::ZlibEncoder};

    use super::CompressionBackend;

    #[derive(Clone, Copy)]
    pub struct Compressor(Compression);

    impl CompressionBackend for Compressor {
        const MAX_LEVEL: u32 = 9;

        // Can't fail, but the libdeflate backend can and both need the same signature.
        fn new(level: u32) -> eyre::Result<Self> {
            Ok(Self(Compression::new(level)))
        }

        fn compress(self, data: &[u8]) -> eyre::Result<Vec<u8>> {
            let mut encoder = ZlibEncoder::new(Vec::new(), self.0);
            encoder
                .write_all(data)
                .context("failed to compress frame chunk")?;
            encoder.finish().context("failed to finish compression")
        }
    }
}

/// Enabled by the `web-libdeflate` feature: compresses noticeably better per CPU spent, at the
/// price of needing a C compiler to build.
#[cfg(feature = "web-libdeflate")]
mod backend {
    use color_eyre::eyre;
    use libdeflater::{CompressionLvl, Compressor as LibdeflateCompressor};

    use super::CompressionBackend;

    #[derive(Clone, Copy)]
    pub struct Compressor(CompressionLvl);

    impl CompressionBackend for Compressor {
        const MAX_LEVEL: u32 = 12;

        fn new(level: u32) -> eyre::Result<Self> {
            // The caller restricts this to the range we accept, so this should not fail.
            let level = CompressionLvl::new(level as i32)
                .map_err(|err| eyre::eyre!("invalid frame compression level {level}: {err:?}"))?;

            Ok(Self(level))
        }

        fn compress(self, data: &[u8]) -> eyre::Result<Vec<u8>> {
            let mut compressor = LibdeflateCompressor::new(self.0);
            // libdeflate compresses in one shot into a caller-provided buffer, so start at the
            // worst case it could need (slightly *more* than the input for incompressible data) and
            // shrink afterwards. Allocating a compressor per chunk is measurably free compared to
            // the compression itself.
            let mut compressed = vec![0; compressor.zlib_compress_bound(data.len())];
            let compressed_len = compressor
                .zlib_compress(data, &mut compressed)
                .map_err(|err| eyre::eyre!("failed to compress frame chunk: {err:?}"))?;
            compressed.truncate(compressed_len);

            Ok(compressed)
        }
    }
}

/// Compresses framebuffer chunks into the zlib streams the browsers decompress.
///
/// Cheap to copy, so every chunk of a frame can get its own handle and compress in parallel.
#[derive(Clone, Copy)]
pub struct FrameCompressor(backend::Compressor);

impl FrameCompressor {
    /// The highest compression level the compiled-in encoder accepts.
    pub const MAX_LEVEL: u32 = backend::Compressor::MAX_LEVEL;

    /// Fails if `level` is not one the compiled-in encoder accepts.
    pub fn new(level: u32) -> eyre::Result<Self> {
        Ok(Self(backend::Compressor::new(level)?))
    }

    /// Zlib-compresses a single chunk of the framebuffer.
    pub fn compress(self, data: &[u8]) -> eyre::Result<Vec<u8>> {
        self.0.compress(data)
    }
}
