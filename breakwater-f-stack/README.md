`apt install dpdk libdpdk-dev make pkg-config libnuma-dev libsystemd-dev ethtool`

Build f-stack
`sbernauer@debian:~/pixelflut/f-stack/lib$ make -j 8`
`sbernauer@debian:~/pixelflut/f-stack/example$ make`

Build breakwater-fstack
`export FF_PATH=/home/sbernauer/pixelflut/f-stack/`
`make`

Start server on 0000:02:00.0:
`sudo dpdk-devbind.py --bind=uio_pci_generic 0000:02:00.0`
`sudo example/helloworld_epoll`

Add clients IP:
`sudo ip link set up dev enp2s0f1`
`sudo ip a a 10.0.0.43/8 dev enp2s0f1`
`sudo ip a a 192.168.1.3/24 dev enp2s0f1`

100 connections, 10s
epoll:      Requests/sec: 166722.4461
            Requests/sec: 163271.0972
No epoll:   Requests/sec: 171561.5445
            Requests/sec: 175499.0806

1000 connections, 10s
epoll:      Requests/sec: 163363.2243
No epoll:   Requests/sec: 162239.4396


# Special experiment for virtual device, as my NIC is not supported
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
