FROM debian:bookworm-slim AS glibc

FROM alpine:3.20 AS guest
RUN apk add --no-cache python3 py3-pip fuse squashfs-tools cpio && \
    python3 -m pip install --break-system-packages --no-cache-dir fusepy==3.0.1
ENV FUSE_LIBRARY_PATH=/usr/lib/libfuse.so.2
WORKDIR /runner
COPY --from=glibc /lib/aarch64-linux-gnu/ /opt/glibc/
COPY --chmod=755 .cache/lucidos /runner/lucidos
COPY guest.py workloads.py ./
ENTRYPOINT ["python3", "/runner/guest.py"]

FROM debian:bookworm-slim AS emulator
RUN apt-get update && apt-get install -y --no-install-recommends qemu-system-arm && \
    rm -rf /var/lib/apt/lists/*
RUN apt-get update && apt-get install -y --no-install-recommends python3 squashfs-tools cpio && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /runner

FROM emulator AS baseline
COPY --from=guest / /guest/
COPY phase11-init.sh /guest/phase11-init
COPY .cache/vmlinuz-virt .cache/modloop-virt /assets/
RUN python3 -c 'import gzip,pathlib,struct; data=pathlib.Path("/assets/vmlinuz-virt").read_bytes(); offset,length=struct.unpack_from("<II",data,8); image=gzip.decompress(data[offset:offset+length]); assert image[56:60] == b"ARMd"; pathlib.Path("/assets/Image").write_bytes(image)' && \
    unsquashfs -d /modules /assets/modloop-virt 'modules/*/kernel/fs/fuse/fuse.ko*' && \
    cp /modules/modules/*/kernel/fs/fuse/fuse.ko* /guest/ && \
    cd /guest && find . -print0 | cpio -o -0 -H newc | gzip -1 >/assets/fixture.cpio.gz
COPY run.py service.py /runner/
ENTRYPOINT ["python3", "/runner/run.py", "--vm", "--expect-abi", "3"]
