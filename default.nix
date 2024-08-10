{ pkgs ? import <nixpkgs> {} }:

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
  ];

  LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
  LIBVNCSERVER_HEADER_FILE = "${pkgs.libvncserver.dev}/include/rfb/rfb.h";

  # Needed for native-display feature
  WINIT_UNIX_BACKEND = "wayland";
  LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath buildInputs}";
  XDG_DATA_DIRS = builtins.getEnv "XDG_DATA_DIRS";
}
