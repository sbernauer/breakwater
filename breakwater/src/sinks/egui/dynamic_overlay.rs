use std::path::Path;

use breakwater_egui_overlay::{DynamicOverlay, Versions};
use log::error;
use snafu::{IntoError, ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("unable to load dynamic library"))]
    LibLoading { source: libloading::Error },

    #[snafu(display("unable to find symbol in dynamic library"))]
    Symbol { source: libloading::Error },

    #[snafu(display("version mismatch"))]
    VersionMismatch,
}

pub enum UiOverlay {
    BuiltIn,
    Dynamic {
        _lib: libloading::Library,
        overlay: DynamicOverlay,
    },
}

/// Safety:
/// UiOverlay never leaves the main thread but tokio does not know that :-)
unsafe impl Send for UiOverlay {}

/// Safety:
/// UiOverlay never leaves the main thread but tokio does not know that :-)
unsafe impl Sync for UiOverlay {}

impl Default for UiOverlay {
    fn default() -> Self {
        Self::BuiltIn
    }
}

impl UiOverlay {
    #[allow(clippy::too_many_arguments)]
    pub fn draw_ui(
        &self,
        viewport_idx: u32,
        ctx: &egui::Context,
        advertised_endpoints: &[String],
        connections: u32,
        ips_v6: u32,
        ips_v4: u32,
        bytes_per_s: u64,
    ) {
        match self {
            UiOverlay::BuiltIn => {
                (breakwater_egui_overlay::DEFAULT_OVERLAY.draw_ui)(
                    breakwater_egui_overlay::DEFAULT_OVERLAY.data,
                    viewport_idx,
                    ctx,
                    advertised_endpoints,
                    connections,
                    ips_v6,
                    ips_v4,
                    bytes_per_s,
                );
            }
            UiOverlay::Dynamic { overlay, .. } => {
                (overlay.draw_ui)(
                    overlay.data,
                    viewport_idx,
                    ctx,
                    advertised_endpoints,
                    connections,
                    ips_v6,
                    ips_v4,
                    bytes_per_s,
                );
            }
        }
    }
}

impl Drop for UiOverlay {
    fn drop(&mut self) {
        match self {
            UiOverlay::BuiltIn => {}
            UiOverlay::Dynamic { overlay, .. } => {
                if !overlay.drop.is_null() {
                    unsafe { (*overlay.drop)(overlay.data) }
                }
            }
        }
    }
}

/// loads a dynamic library for a custom overlay and checks its version
pub fn load_and_check(dylib_path: impl AsRef<Path>) -> Result<UiOverlay, Error> {
    unsafe {
        let dylib =
            libloading::Library::new(dylib_path.as_ref().as_os_str()).context(LibLoadingSnafu)?;
        let dylib_versions: libloading::Symbol<fn() -> Versions> =
            dylib.get(b"versions").context(SymbolSnafu)?;

        let dylib_versions = dylib_versions();

        if dylib_versions != breakwater_egui_overlay::VERSIONS {
            error!(
                "dylib version ({dylib_versions:?}) do not match our version ({:?})",
                breakwater_egui_overlay::VERSIONS
            );
            return Err(VersionMismatchSnafu.into_error(snafu::NoneError));
        }

        let dylib_new: libloading::Symbol<fn() -> DynamicOverlay> =
            dylib.get(b"new").context(SymbolSnafu)?;

        let overlay = dylib_new();

        if !overlay.new.is_null() {
            (*overlay.new)(overlay.data)
        };

        Ok(UiOverlay::Dynamic {
            _lib: dylib,
            overlay,
        })
    }
}
