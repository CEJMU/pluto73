# Cross-building the pluto73 app on macOS, Linux, and Windows

`make app` expects a Linux host with the Linaro toolchain in `/opt/toolchains`
and the buildroot sysroot at `plutoplus/plutosdr-fw/buildroot/output/...`. However, the toolchain is a Linux binary, and the buildroot sysroot only appears after a full firmware build (which needs Vivado).
This directory supplies an `arm-linux-gnueabihf` toolchain and the required library bindings inside a docker container.

Only the Rust app is covered here. FPGA/firmware targets (`bitstream`, `firmware`, `bake`) still need Vivado on Linux.

## Usage

1. **Build the cross-build Docker image** (run once):

```bash
./cross-compile/build-image.sh      # Linux / macOS
cross-compile\build-image.bat       # Windows
```

2. **Build & Deploy via Makefile**:

```bash
make app-cross                      # cross-compiles binaries inside Docker container
make deploy-cross                   # Docker build + deploy to 192.168.2.1
make deploy-cross TARGET_IP=192.168.178.183 # Docker build + deploy to custom IP
make run-cross                      # Docker build + deploy + execute remotely on device
```

For native Linux builds (with local toolchain in `/opt/toolchains`), use standard `make app`, `make deploy`, or `make run`.

`build-image.sh` needs the Linaro tarball which is downloaded into `cross-compile/` automatically on first run — checksum-verified.
To supply it manually instead, download **7.5.0-2019.12, host `x86_64`, target `arm-linux-gnueabihf`** from [ARM's Legacy Linaro GNU Toolchains](https://developer.arm.com/Downloads/-/Legacy%20Linaro%20GNU%20Toolchains); place it in `cross-compile/`, or pass its path as an argument.

`make.sh` passes any Makefile target through, but only build targets make sense in the container.

## libs/

Taken directly off the device, so they match what the app will actually run against. `libiio-sys` uses pre-generated bindings and only emits `cargo:rustc-link-lib=iio`, so no headers or pkg-config are involved.
The linker still follows `libiio.so`'s `DT_NEEDED` entries, so its whole dependency closure has to be present or the final link fails with undefined `libusb_*` / `avahi_*` / `sp_*` symbols.

Files are named by SONAME, which is what the linker searches for. To refresh them (e.g. after a firmware update changes libiio), with the device reachable over SSH:

```bash
ssh root@<device> "cat /usr/lib/libiio.so.0.25" > libs/libiio.so.0.25
```

and likewise for any dependency. The device has no `sftp-server`, so plain `scp` fails. Use `ssh cat` as above, or `scp -O`.

To re-check the closure after a change:

```bash
docker run --rm --platform linux/amd64 -v "$PWD/libs:/dl:ro" pluto73-cross:latest bash -c '
SR=/opt/toolchain/arm-linux-gnueabihf/libc
for f in /dl/*; do arm-linux-gnueabihf-readelf -d "$f" | grep NEEDED | sed "s/.*\[\(.*\)\]/\1/" | while read n; do
  [ -e "$SR/lib/$n" ] || [ -e "$SR/usr/lib/$n" ] || [ -e "/dl/$n" ] || echo "MISSING $n (needed by $(basename $f))"
done; done'
```
