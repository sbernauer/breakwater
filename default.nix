{ nixpkgs ? import <nixpkgs> {} }:

nixpkgs.mkShell {
  buildInputs = [
    nixpkgs.pkg-config
    nixpkgs.clang
    nixpkgs.libclang
    nixpkgs.libvncserver
    nixpkgs.libvncserver.dev
  ];

  LIBCLANG_PATH = "${nixpkgs.libclang.lib}/lib";
  LIBVNCSERVER_HEADER_FILE = "${nixpkgs.libvncserver.dev}/include/rfb/rfb.h";
}
