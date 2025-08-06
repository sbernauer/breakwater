{
  nixpkgs ? import <nixpkgs> {},
  nixpkgsUnstable ? import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/nixos-unstable.tar.gz") {},
}:

nixpkgs.mkShell {
  buildInputs = [
    nixpkgs.pkg-config
    nixpkgs.dpdk
    # nixpkgsUnstable.dpdk # Uncomment to use dpdk from nixpkgs-unstable
    nixpkgs.openssl

    nixpkgs.numactl
    nixpkgs.zlib
    nixpkgs.jemalloc
    nixpkgs.jansson
    nixpkgs.libpcap
    nixpkgs.libnl
    nixpkgs.libelf
    # nixpkgs.libnfnetlink
  ];
}
