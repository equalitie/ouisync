# Ouinode packages for OpenWrt

```sh
make -C openwrt                          # both formats for all four targets
make -C openwrt ipk ARCH=aarch64         # opkg package for OpenWrt
make -C openwrt apk ARCH='mips mipsle'   # apk package for OpenWrt
make -C openwrt ipk apk ARCH=armv7       # compile once, package both formats
```
