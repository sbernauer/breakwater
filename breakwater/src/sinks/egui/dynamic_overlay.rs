use std::path::Path;

use breakwater_egui_overlay::{DynamicOverlay, Versions};
use color_eyre::eyre::{self, Context};
use tracing::instrument;

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
#[instrument(err)]
pub fn load_and_check(dylib_path: impl AsRef<Path> + std::fmt::Debug) -> eyre::Result<UiOverlay> {
    unsafe {
        let dylib = libloading::Library::new(dylib_path.as_ref().as_os_str())
            .context("failed to load dynamic library")?;
        let dylib_versions: libloading::Symbol<fn() -> Versions> = dylib
            .get(b"versions")
            .context("failed to locate 'versions()' function")?;

        let dylib_versions = dylib_versions();

        if dylib_versions != breakwater_egui_overlay::VERSIONS {
            tracing::error!(
                "dylib versions ({dylib_versions:?}) do not match our versions ({:?})",
                breakwater_egui_overlay::VERSIONS
            );
            eyre::bail!("dynamic overlay version check failed");
        }

        let dylib_new: libloading::Symbol<fn() -> DynamicOverlay> = dylib
            .get(b"new")
            .context("failed to locate 'new()' function")?;

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
