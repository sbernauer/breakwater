# NDI is unfree, so we need to allow unfree packages for the `ndi` feature.
{ pkgs ? import <nixpkgs> { config.allowUnfree = true; } }:

let
  # The ndi-sdk-sys crate needs NDI 6 (it references NDIlib_frame_type_source_change,
  # which only exists from v6 onwards; v5 only has status_change).
  # Upstream re-published the tarball, so the hash pinned in nixpkgs is stale and we
  # override it here.
  ndi = pkgs.ndi-6.overrideAttrs (old: {
    src = pkgs.fetchurl {
      url = "https://downloads.ndi.tv/SDK/NDI_SDK_Linux/Install_NDI_SDK_v6_Linux.tar.gz";
      hash = "sha256-8DFPJFRG3vxIi2POtGiazxqWWu79ray3BXG7IWqMwYM=";
    };
  });
in
pkgs.stdenv.mkDerivation rec {
  name = "dev-shell";

  buildInputs = with pkgs; [
    pkg-config
    clang
    libclang
    libvncserver
    libvncserver.dev

    # Needed for native-display feature
    wayland
    libGL
    libxkbcommon

    # Needed for ndi feature
    ndi
  ];

  LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
  LIBVNCSERVER_HEADER_FILE = "${pkgs.libvncserver.dev}/include/rfb/rfb.h";

  # ndi-sdk-sys's build.rs looks for Processing.NDI.Lib.h here
  NDI_HEADER_DIR = "${ndi}/include";

  # Needed for native-display feature
  WINIT_UNIX_BACKEND = "wayland";
  LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath buildInputs}";
  XDG_DATA_DIRS = builtins.getEnv "XDG_DATA_DIRS";
}
