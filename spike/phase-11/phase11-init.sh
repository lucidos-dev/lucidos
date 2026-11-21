#!/bin/sh
set -eu
mount -t devtmpfs devtmpfs /dev
mount -t proc proc /proc
mount -t sysfs sysfs /sys
chmod 600 /dev/ttyAMA0
ip link set lo up
stty -F /dev/ttyAMA0 raw -echo
exec </dev/ttyAMA0 >/dev/ttyAMA0 2>&1
module=$(find / -maxdepth 1 -name 'fuse.ko*' -print -quit)
case "$module" in
    *.gz) gzip -d "$module"; module=${module%.gz} ;;
    *.xz) xz -d "$module"; module=${module%.xz} ;;
esac
insmod "$module"
export FUSE_LIBRARY_PATH=/usr/lib/libfuse.so.2
exec python3 /runner/guest.py
