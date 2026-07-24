# Pluto SSB Transceiver

Firmware, FPGA, and host-application source for a single-sideband (SSB) transceiver built on the Pluto+ SDR. Developed as a bachelor thesis in the **Computer Engineering Group (Chair of Computer Science XVII), Julius-Maximilians-Universität Würzburg**.

The design adds a full digital up/down-conversion datapath and an audio DMA subsystem to the Pluto FPGA, driven by a Rust application on the on-board ARM core.

---

## Attribution & provenance

This project is layered on top of two upstream firmware bases. They are pulled in as git submodules at fixed commits, and this repository contains only **its own additions** (as source files) and **its own modifications to the upstream trees** (as patches in `patches/`).

```
analogdevicesinc/plutosdr-fw   (ADI base firmware)
        └── plutoplus/plutoplus  (Pluto+ platform mods: 2T2R, GbE, SD card)
                └── THIS REPO      (SSB transceiver: custom DSP block design + drivers + app)
```

The `patches/` diffs were generated **relative to the Pluto+ baseline** (ADI + Pluto+'s own patches applied), so they contain only this project's changes. None of Pluto+'s or ADI's work is attributed here. The upstream licenses (in the submodules) continue to apply to the submodule contents.

> **Disclaimer:** The web frontend (`static/`) was created with the help of AI tools.

---

## Repository layout

```
.
├── plutoplus/                 # submodule: Pluto+ firmware (which submodules ADI's plutosdr-fw)
├── patches/                   # this project's edits to the upstream firmware trees
│   ├── hdl.diff               #   - projects/pluto/system_bd.tcl -> custom-BD wrapper (see below)
│   ├── linux.diff             #   - device tree: reserved DMA buffer, DSP-config GPIO, audio DMA (generic-uio); defconfig
│   ├── u-boot-xlnx.diff       #   - u-boot dts / defconfig / zynq-common.h
│   └── buildroot.diff         #   - post-build.sh: bake the app/frontend/certs into /opt/pluto + autostart; motd
├── hdl_bd/
│   └── system_bd_design.tcl   # the FPGA block design (write_bd_tcl export)
├── hdl_modules/               # custom Verilog instantiated by the block design
│   ├── burst_gate.v  dc_block_cmpy.v  dsp_mux_rx.v  dsp_mux_tx.v
│   └── iq_packer.v   tx_strobe_gen.v
├── scripts/
│   ├── apply.sh               # apply this project's patches onto the Pluto+ baseline
│   ├── revert.sh              # reverse them
│   └── collect.sh             # regenerate patches/*.diff from the working trees (isolated from Pluto+'s)
├── src/  Cargo.toml  ...        # Rust host application  (see "Host application")
├── Makefile                   # firmware + app build orchestration (`make help`)
├── TEST.md                    # on-device diagnostic test suite reference
├── LICENSE                    # MIT (applies to this repo's own code; submodules keep their own)
└── README.md
```

### How the FPGA block design is integrated

The Pluto block design is a Vivado `write_bd_tcl` export (`hdl_bd/system_bd_design.tcl`). Rather than vendoring it into the submodule, `patches/hdl.diff` replaces the stock `projects/pluto/system_bd.tcl` with a small **wrapper** that:

1. registers `hdl_modules/*.v` as project sources
2. `source`s `hdl_bd/system_bd_design.tcl` into the design that ADI's `adi_project_create` has already created.

This keeps the ~1400-line generated design as a **first-class, refreshable file** in this repo (not buried in a patch), while the change to the upstream tree stays a handful of lines. Verified end-to-end: the design builds and `validate_bd_design` passes in the real ADI project flow.


Note: To keep Vivado from auto-inferring bus interfaces on generic control/status port names, since it matches names like `rst_n` or `*valid*` against known interface patterns and groups them, some ports in hdl_modules are marked `X_INTERFACE_IGNORE`.

---

## What was changed vs. stock Pluto+

**FPGA block design:**

- **Added** - a complete DUC/DDC + audio path: 4x CIC compilers, 2x FIR compilers, 2x DDS compilers, 2x complex multipliers, a third DMA (`axi_dmac_audio`), 2x AXI GPIO, glue (xlslice / xlconcat / util_vector_logic), and 6 custom Verilog modules (`burst_gate`, `dc_block_cmpy`, `dsp_mux_rx`, `dsp_mux_tx`, `iq_packer`, `tx_strobe_gen`).
- **Removed** - the stock ADI fixed-rate filter chain: `fir_decimation_0/1`, `fir_interpolation_0/1`, their `decim_slice`/`interp_slice` and `logic_and`/`logic_or` gates, and the now-unused `GND_32`/`VCC_1` constants.
- **Rewired** - the ADC/DAC datapaths are rerouted through the new chain.

**Kernel / boot** (`patches/linux.diff`, `patches/u-boot-xlnx.diff`):
reserved-memory buffer for Rust-owned DMA, a `generic-uio` audio-DMA node (kept out of kernel management), and matching defconfig / u-boot changes.

---

## Building

### Prerequisites

Host packages (Debian/Ubuntu) - the firmware build (buildroot / kernel / u-boot) needs:

```bash
sudo apt-get install git build-essential fakeroot libncurses5-dev libssl-dev ccache
sudo apt-get install dfu-util u-boot-tools device-tree-compiler mtools
sudo apt-get install bc python3 cpio zip unzip rsync file wget
sudo apt-get install libgmp-dev libmpfr-dev libmpc-dev
```

Toolchains & tools:

- **Vivado 2022.2** (e.g. `/tools/Xilinx/Vivado/2022.2/`). Point `VIVADO_SETTINGS` at its `settings64.sh`. Set it at the top of the `Makefile`, or `export VIVADO_SETTINGS=/tools/Xilinx/Vivado/2022.2/settings64.sh`.
- **Linaro `arm-linux-gnueabihf` GCC 7.3.1-2018.05**: buildroot, Linux and u-boot are built with this external toolchain, because the AMD/Xilinx GCC bundled with Vivado/Vitis is incompatible with Buildroot. Download it from [releases.linaro.org .../7.3-2018.05/arm-linux-gnueabihf](https://developer.arm.com/Downloads/-/Legacy%20Linaro%20GNU%20Toolchains) and set its path at the top of the `Makefile` (`CROSS_COMPILE_PATH` / `TOOLCHAIN_DIR`).
- **Rust**, with the device target for the host app: `rustup target add armv7-unknown-linux-gnueabihf`.

Clone recursively (this repo submodules Pluto+, which submodules ADI's plutosdr-fw):

```bash
git clone --recursive <this-repo-url>
```

Run `make help` for all targets.

### One command

```bash
make all      # fetch submodules -> apply Pluto+ + custom patches -> firmware with the app baked in
```

### Flashing artifacts

The build produces the same set as the upstream plutosdr-fw `make`, in `plutoplus/plutosdr-fw/build/`:

| File | Flash method |
|---|---|
| `pluto.frm` | mass-storage / web-UI firmware update |
| `pluto.dfu`, `boot.dfu`, `uboot-env.dfu` | `dfu-util` - on device `pluto_reboot ram`, then `dfu-util -a 1 -D pluto.dfu` |
| `boot.frm` | bootloader image |
| `<prefix>-jtag-bootstrap-*.zip` | JTAG recovery |

(The `.dfu` files need `dfu-util`/`dfu-suffix`; they are skipped automatically if it isn't installed.)

### TLS certificates

The app serves the web UI over HTTPS (443) and HTTP (8080). Drop your own `cert.pem`/`key.pem` in the repo root, or generate a self-signed pair:

```bash
make certs    # generates cert.pem/key.pem (CN=192.168.2.1), only if none are present
```

`make all` / `make bake` / `make deploy` invoke this automatically; whatever `cert.pem`/`key.pem` are present get baked into the image (`/opt/pluto`). With no certs, the app serves HTTP only.

> **Security note:** the web UI has **no authentication**. Anyone who can reach the device's
> network interface can control the receiver and key the transmitter. The Pluto's USB network is
> a point-to-point link, but take care before bridging the device onto a larger network.

### Step by step (equivalent)

```bash
make setup       # = submodules + baseline (Pluto+'s patches) + patch (ours)
make firmware    # synth -> impl -> bitstream -> xsa -> ... -> flashing artifacts (no app)
make bake        # firmware + cross-compile the app + re-image with app/frontend/certs baked in
```

### Host application (iterate without re-flashing)

```bash
make deploy      # cross-compile the Rust app + scp binary, static/ and TLS certs to 192.168.2.1
make run         # deploy + run on-device
```

> The Rust app links against `libiio` from the firmware **buildroot sysroot**, so a
> firmware build must precede the app cross-compile. `patch` must follow `baseline`,
> since our diffs are cut against the Pluto+ baseline.

### Cleaning up

```bash
make clean       # remove build artifacts (firmware build/, Vivado project, cargo target); keeps patches
make distclean   # clean + revert all patches, resetting the submodule trees to pristine ADI
```

## Refreshing the block design

After changing the design in the Vivado GUI, re-export it. Do **not** hand-edit the export or the wrapper:

```tcl
write_bd_tcl <this_repo>/hdl_bd/system_bd_design.tcl
```

For edits to *tracked* firmware files (device trees, constraints, the wrapper), run `make collect` to regenerate the patches.
