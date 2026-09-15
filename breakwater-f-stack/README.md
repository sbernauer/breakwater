`apt install dpdk libdpdk-dev make pkg-config libnuma-dev libsystemd-dev ethtool`

### Build f-stack

Hint: The following `default.nix` might be helpful

```
{
  nixpkgs ? import <nixpkgs> {},
  nixpkgsUnstable ? import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/nixos-unstable.tar.gz") {},
}:

nixpkgs.mkShell {
  buildInputs = [
    nixpkgs.pkg-config
    nixpkgs.meson
    nixpkgs.ninja

    nixpkgs.dpdk
    nixpkgs.openssl # Needed by f-stack
    nixpkgs.numactl

    # Needed by the redis example app
    nixpkgs.zlib
    nixpkgs.jemalloc
    nixpkgs.jansson
    nixpkgs.libpcap
    # nixpkgs.libnfnetlink
    nixpkgs.libnl
    nixpkgs.libelf

    # nixpkgsUnstable.dpdk # Uncomment to use dpdk from nixpkgs-unstable
    # nixpkgs.imagemagick
    nixpkgs.python312Packages.pyelftools # needed for dpdk-pmdinfo.py

    # For debugging
    # nixpkgs.gdb
  ];
}
```

`sbernauer@debian:~/pixelflut/f-stack/lib$ make -j 8`

`sbernauer@debian:~/pixelflut/f-stack/example$ make # Only needed for testing`

### Build breakwater-f-stack

`export FF_PATH=/home/sbernauer/pixelflut/f-stack/`

You also need to build the `breakwater-parser-c-bindings`, so that the C program can link and use the `breakwater-parser` Rust functions.
You can do that using `cargo build --release -p breakwater-parser-c-bindings` from the git root.

Afterwards, in this folder, run `make`.

### Run breakwater-f-stack server

#### Prerequisites

You need to have a breakwater server running using `cargo run --release -- --shared-memory-name breakwater --vnc`.
This opens the shared memory region for the breakwater-f-stack server to write into.
You can connect via VNC or append `--native-display` to the breakwater call to get a graphical output.

#### Run breakwater-f-stack

Start server on 0000:02:00.0:

`sudo modprobe uio_pci_generic`

`sudo dpdk-devbind.py --bind=uio_pci_generic 0000:02:00.0`

`sudo bash -c 'echo 1024 > /sys/devices/system/node/node0/hugepages/hugepages-2048kB/nr_hugepages'`

As we linked against the dynamic library of `breakwater-parser-c-bindings`, we need to specify the `LD_LIBRARY_PATH` here:

`sudo LD_LIBRARY_PATH=../target/release build/breakwater-f-stack`

Add clients IP:

`sudo ip link set up dev enp1s0f1`

`sudo ip a a 10.0.0.42/8 dev enp1s0f1`

`sudo ip a a 192.168.1.3/24 dev enp1s0f1`

`ping 192.168.1.2` should now succeed (if it doesn't check e.g. `dmesg`).

With a single desktop core from 2011 we can get 4.5 Gbit/s, pretty slow!
Normal breakwater on a single core on the same machine reaches 12G via loopback.

### Run on multiple cores

Disclaimer: While I got it to run on multiple cores I could not get it faster than running on a single core yet!

1. Edit `lcore_mask` in `config.ini`. Hint: It's hexadecimal.
2. Start multiple processes using `sudo ./start.sh`
3. Stop all running processes using `sudo pkill -f breakwater-f-stack`

### Special experiment for virtual device, as my NIC is not supported

f-stack patches:

```patch
diff --git a/lib/ff_config.c b/lib/ff_config.c
index 18380919..a9d74099 100644
--- a/lib/ff_config.c
+++ b/lib/ff_config.c
@@ -1009,6 +1009,9 @@ dpdk_args_setup(struct ff_config *cfg)
 
     }
 
+    // dpdk_argv[n++] = strdup("--vdev=net_pcap0,iface=lo");
+    dpdk_argv[n++] = strdup("--vdev=net_tap");
+
     if (cfg->dpdk.nb_vdev) {
         for (i=0; i<cfg->dpdk.nb_vdev; i++) {
             sprintf(temp, "--vdev=virtio_user%d,path=%s",
diff --git a/lib/ff_dpdk_if.c b/lib/ff_dpdk_if.c
index f6fe50e5..a3c17e74 100644
--- a/lib/ff_dpdk_if.c
+++ b/lib/ff_dpdk_if.c
@@ -668,8 +668,8 @@ init_port_start(void)
                     printf("Use symmetric Receive-side Scaling(RSS) key\n");
                     rsskey = symmetric_rsskey;
                 }
-                port_conf.rx_adv_conf.rss_conf.rss_key = rsskey;
-                port_conf.rx_adv_conf.rss_conf.rss_key_len = rsskey_len;
+                // port_conf.rx_adv_conf.rss_conf.rss_key = rsskey;
+                // port_conf.rx_adv_conf.rss_conf.rss_key_len = rsskey_len;
                 port_conf.rx_adv_conf.rss_conf.rss_hf &= dev_info.flow_type_rss_offloads;
                 if (port_conf.rx_adv_conf.rss_conf.rss_hf !=
                         RTE_ETH_RSS_PROTO_MASK) {
```

`sudo ip a a 192.168.1.3/24 dev dtap0`
