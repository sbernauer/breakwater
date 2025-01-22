pub use eframe;
pub use egui;

use egui::Margin;

/// rustc version that compiled this crate
//
// Currently it's not supported to read the rust version from a cargo env,
// so we need to gather it ourselves.
// See https://github.com/rust-lang/cargo/issues/4408 for details
const RUSTC_VERSION: *const std::ffi::c_char = concat!(
    include_str!(concat!(env!("OUT_DIR"), "/RUSTC_VERSION.txt")),
    "\0"
)
.as_ptr() as _;

const BREAKWATER_VERSION: *const std::ffi::c_char =
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as _;

#[repr(C)]
#[derive(Clone)]
pub struct Versions {
    pub rustc: *const std::ffi::c_char,
    pub breakwater: *const std::ffi::c_char,
}

impl std::fmt::Debug for Versions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::ffi::CStr;

        #[derive(Debug)]
        #[allow(dead_code)]
        struct Versions<'a> {
            rustc: &'a CStr,
            breakwater: &'a CStr,
        }

        unsafe {
            Versions {
                rustc: CStr::from_ptr(self.rustc),
                breakwater: CStr::from_ptr(self.breakwater),
            }
            .fmt(f)
        }
    }
}

pub const VERSIONS: Versions = Versions {
    rustc: RUSTC_VERSION,
    breakwater: BREAKWATER_VERSION,
};

impl PartialEq for Versions {
    fn eq(&self, other: &Self) -> bool {
        use std::ffi::CStr;

        unsafe {
            CStr::from_ptr(self.rustc).eq(CStr::from_ptr(other.rustc))
                && CStr::from_ptr(self.breakwater).eq(CStr::from_ptr(other.breakwater))
        }
    }
}

pub type New = extern "C" fn(*mut std::ffi::c_void);
pub type Drop = extern "C" fn(*mut std::ffi::c_void);
#[allow(improper_ctypes_definitions)] // only called from rust
pub type DrawUi = extern "C" fn(
    data: *mut std::ffi::c_void,
    viewport_idx: u32,
    ctx: &egui::Context,
    advertised_endpoints: &[String],
    connections: u32,
    ips: u32,
    legacy_ips: u32,
    bytes_per_s: u64,
);

#[repr(C)]
pub struct DynamicOverlay {
    pub data: *mut std::ffi::c_void,
    pub new: *const New,
    pub draw_ui: DrawUi,
    pub drop: *const Drop,
}

/// 38c3 colors
#[allow(dead_code)]
mod colors {
    use egui::Color32;

    pub const COLOR_PRIMARY: Color32 = Color32::from_rgb(0xFF, 0x50, 0x53);
    pub const COLOR_HIGHLIGHT: Color32 = Color32::from_rgb(0xFE, 0xF2, 0xFF);
    pub const COLOR_ACCENT_A: Color32 = Color32::from_rgb(0xB2, 0xAA, 0xFF);
    pub const COLOR_ACCENT_B: Color32 = Color32::from_rgb(0x6A, 0x5F, 0xDB);
    pub const COLOR_ACCENT_C: Color32 = Color32::from_rgb(0x29, 0x11, 0x4C);
    pub const COLOR_ACCENT_D: Color32 = Color32::from_rgb(0x26, 0x1A, 0x66);
    pub const COLOR_ACCENT_E: Color32 = Color32::from_rgb(0x19, 0x0B, 0x2F);
    pub const COLOR_BACKGROUND: Color32 = Color32::from_rgb(0x0F, 0x00, 0x0A);
}

pub const DEFAULT_OVERLAY: DynamicOverlay = DynamicOverlay {
    data: std::ptr::null_mut(),
    new: std::ptr::null(),
    draw_ui: draw_ui as _,
    drop: std::ptr::null(),
};

#[allow(improper_ctypes_definitions)]
extern "C" fn draw_ui(
    _: *mut std::ffi::c_void,
    viewport_idx: u32,
    ctx: &egui::Context,
    advertised_endpoints: &[String],
    connections: u32,
    ips: u32,
    legacy_ips: u32,
    bytes_per_s: u64,
) {
    use colors::*;

    // only display on first viewport
    if viewport_idx > 0 {
        return;
    }

    let stats_frame = egui::Frame {
        fill: COLOR_BACKGROUND.gamma_multiply(0.7),
        stroke: egui::Stroke::new(1.0, COLOR_PRIMARY),
        rounding: egui::Rounding::same(10.0),
        shadow: eframe::epaint::Shadow::default(),
        inner_margin: Margin::same(12.0),
        outer_margin: Margin::same(12.0),
    };

    egui::Area::new(egui::Id::new("overlay_area"))
        .movable(true)
        .fixed_pos(egui::pos2(20.0, 20.0)) // Initial position on the screen
        .show(ctx, |ui| {
            stats_frame.show(ui, |ui| {
                ui.label(
                    egui::RichText::new("breakwater | Pixelflut")
                        .size(48.0)
                        .color(COLOR_HIGHLIGHT),
                );
                ui.separator();

                egui::Grid::new(egui::Id::new("stats_header_grid")).show(ui, |ui| {
                    for ep in advertised_endpoints {
                        ui.label(
                            egui::RichText::new(ep)
                                .color(COLOR_HIGHLIGHT)
                                .size(32.0)
                                .strong(),
                        );
                        ui.end_row();
                    }
                });

                ui.separator();
                egui::Grid::new(egui::Id::new("stats_metrics_grid")).show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Connections: ")
                            .color(COLOR_HIGHLIGHT)
                            .size(24.0),
                    );
                    ui.label(
                        egui::RichText::new(format!("{}", connections))
                            .color(COLOR_HIGHLIGHT)
                            .size(24.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("IPv6: ")
                            .color(COLOR_HIGHLIGHT)
                            .size(24.0),
                    );
                    ui.label(
                        egui::RichText::new(format!("{}", ips))
                            .color(COLOR_HIGHLIGHT)
                            .size(24.0)
                            .strong(),
                    );
                    ui.end_row();
                    ui.label(
                        egui::RichText::new("RX: ")
                            .color(COLOR_HIGHLIGHT)
                            .size(24.0),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{:.2} GBit/s       ",
                            (bytes_per_s * 8) as f32 / 1024.0 / 1024.0 / 1024.0
                        ))
                        .color(COLOR_HIGHLIGHT)
                        .size(24.0)
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new("IPv4: ")
                            .color(COLOR_HIGHLIGHT)
                            .size(24.0),
                    );
                    ui.label(
                        egui::RichText::new(format!("{}", legacy_ips))
                            .color(COLOR_HIGHLIGHT)
                            .size(24.0)
                            .strong(),
                    );
                    ui.end_row();
                });
            });
        });
}
