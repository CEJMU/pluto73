# =============================================================================
# Pluto SSB Transceiver -- top-level Makefile
# Bundles (a) firmware/FPGA patch management and (b) the Rust host application.
# See README.md for the full build walkthrough and prerequisites.
#
# Environment-specific paths (toolchain, Vivado) are collected at the top --
# adjust them to your machine if they differ.
# =============================================================================

ROOT      := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
SHELL     := /bin/bash
FW_DIR    := $(ROOT)plutoplus/plutosdr-fw
SYSROOT   := $(FW_DIR)/buildroot/output/host/arm-buildroot-linux-gnueabihf/sysroot

# --- Versioning & Release ----------------------------------------------------
FW_VERSION       = $(shell git -C $(FW_DIR) describe --abbrev=4 --dirty --always --tags)
APP_VERSION      = $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(ROOT)Cargo.toml)
RELEASE_ZIP_NAME = plutoplus-fw-$(FW_VERSION)-$(APP_VERSION).zip


# --- Toolchains (adjust to your environment) ---------------------------------
CROSS_COMPILE_PATH := /opt/toolchains/gcc-linaro-7.3.1-2018.05-x86_64_arm-linux-gnueabihf/bin
TOOLCHAIN_DIR      := /opt/toolchains/gcc-linaro-7.3.1-2018.05-x86_64_arm-linux-gnueabihf
VIVADO_SETTINGS    := /tools/Xilinx/Vivado/2022.2/settings64.sh
export PATH        := $(CROSS_COMPILE_PATH):$(PATH)
export TOOLCHAIN_DIR
export VIVADO_SETTINGS

# --- Target device -----------------------------------------------------------
TARGET_IP    := 192.168.2.1
TARGET_USER  := root
TARGET_DIR   := /root/
BINARY_NAME  := pluto
TARGET_ARCH  := armv7-unknown-linux-gnueabihf

# --- Rust cross-compile config (uses the firmware buildroot sysroot for libiio)
export PKG_CONFIG_LIBDIR        = $(SYSROOT)/usr/lib/pkgconfig:$(SYSROOT)/usr/share/pkgconfig
export PKG_CONFIG_SYSROOT_DIR   = $(SYSROOT)
export PKG_CONFIG_ALLOW_CROSS   = 1
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUSTFLAGS = -C target-cpu=cortex-a9 -C target-feature=+neon -C link-arg=--sysroot=$(SYSROOT) -L $(SYSROOT)/usr/lib -L $(SYSROOT)/lib
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER = arm-linux-gnueabihf-gcc
export CC_armv7_unknown_linux_gnueabihf  = arm-linux-gnueabihf-gcc
export CXX_armv7_unknown_linux_gnueabihf = arm-linux-gnueabihf-g++

.DEFAULT_GOAL := help

# =============================================================================
# One-shot: from a fresh clone to a flashable firmware image
# =============================================================================
.PHONY: all setup
all: setup bake           ## From a fresh clone: fetch + patch + build firmware with the app baked in
	@echo
	@echo "==> Firmware ready (app baked in): $(FW_DIR)/build/pluto.frm"

setup:                    ## Fetch submodules and apply the Pluto+ + custom patch stack
	$(MAKE) submodules
	$(MAKE) baseline
	$(MAKE) patch

# =============================================================================
# Setup steps (idempotent -- safe to re-run)
# =============================================================================
.PHONY: submodules baseline patch unpatch collect
submodules:               ## Fetch Pluto+ and its nested plutosdr-fw submodules
	git submodule update --init --recursive

baseline:                 ## Apply Pluto+'s own patches onto the ADI base (skips if already applied)
	@if git -C $(FW_DIR)/hdl apply -R --check $(ROOT)plutoplus/patches/hdl.diff >/dev/null 2>&1; then \
	  echo "Pluto+ baseline already applied, skipping."; \
	else \
	  cd plutoplus && bash scripts/apply.sh; \
	fi

patch:                    ## Apply THIS project's custom patches on top (skips if already applied)
	bash scripts/apply.sh

unpatch:                  ## Reverse this project's custom patches
	bash scripts/revert.sh

collect:                  ## Regenerate patches/*.diff from the current submodule trees
	bash scripts/collect.sh

# =============================================================================
# FPGA / firmware  (fully headless -- no Vivado GUI step required)
# =============================================================================
# Flashing artifacts, matching what the upstream plutosdr-fw `make` produces:
#   pluto.frm       -- MSD / web-UI update image
#   pluto.dfu, boot.dfu, uboot-env.dfu -- dfu-util flashing
#   boot.frm        -- bootloader image
#   jtag-bootstrap  -- JTAG recovery bundle (zip)
# We build these explicitly, never the fw `all` target -- its clean-build does `rm -rf build/*`
# and would force a full Vivado re-synth. The .dfu variants need dfu-suffix (dfu-util) and are
# dropped automatically when it is absent, mirroring the fw Makefile's own conditional.
FLASH_TARGETS := build/pluto.frm build/boot.frm jtag-bootstrap
ifneq (,$(shell command -v dfu-suffix 2>/dev/null))
FLASH_TARGETS += build/pluto.dfu build/uboot-env.dfu build/boot.dfu
endif

# --- HDL source freshness -----------------------------------------------------
# Removes the stale xsa at BOTH levels when any HDL source is newer, so the next Vivado sub-make re-fires a fresh bitstream run.
XSA       := $(FW_DIR)/build/system_top.xsa
INNER_XSA := $(FW_DIR)/hdl/projects/pluto/pluto.sdk/system_top.xsa
HDL_SRCS  := $(wildcard hdl_modules/*.v) $(wildcard hdl_bd/*.tcl)

.PHONY: hdl-check
hdl-check:
	@if [ -f $(XSA) ] && [ -n "$$(find $(HDL_SRCS) -newer $(XSA) 2>/dev/null)" ]; then \
	  echo "==> HDL sources changed since last bitstream -- forcing a fresh Vivado run"; \
	  rm -f $(XSA) $(INNER_XSA); \
	fi

.PHONY: bitstream firmware bitstream-incr firmware-incr
bitstream: hdl-check      ## Synth+impl the FPGA project and export system_top.xsa (LONG; needs Vivado)
	# Drives the fw Makefile's own xsa rule, which runs the Vivado project build
	# (system_project.tcl -> our wrapper system_bd.tcl -> hdl_bd/ + hdl_modules),
	# generates the bitstream, and write_hw_platform's system_top.xsa into build/.
	source $(VIVADO_SETTINGS) && $(MAKE) -C $(FW_DIR) build/system_top.xsa

firmware: hdl-check       ## Build all flashable images end-to-end (frm + dfu + boot; auto-builds bitstream+xsa first)
	# Chains synth -> impl -> bitstream -> xsa -> fsbl/u-boot/kernel/dtb/rootfs -> itb -> the
	# flashing artifacts, with no manual steps.
	source $(VIVADO_SETTINGS) && $(MAKE) -C $(FW_DIR) $(FLASH_TARGETS)
	@echo "Flashing artifacts ready in $(FW_DIR)/build/ ($(FLASH_TARGETS))"

.PHONY: bake certs
bake: firmware certs      ## Cross-build the app and bake it (+ frontend + TLS certs) into the firmware image
	# `firmware` (above) builds the buildroot sysroot; now cross-compile the app against it and
	# re-run the image build. rootfs.cpio.gz is .PHONY in the fw Makefile, so this re-runs
	# board/pluto/post-build.sh, which installs the freshly built binary, static/, and
	# cert.pem/key.pem into /opt/pluto and adds the S99pluto autostart script.
	$(MAKE) app
	source $(VIVADO_SETTINGS) && $(MAKE) -C $(FW_DIR) $(FLASH_TARGETS)
	@echo "Flashing artifacts (app baked in) ready in $(FW_DIR)/build/ ($(FLASH_TARGETS))"

.PHONY: release sysroot
release: bake             ## Build baked firmware + package release zip (boot.dfu, boot.frm, pluto.dfu, pluto.frm, uboot-env.dfu)
	cd $(FW_DIR)/build && zip -j $(RELEASE_ZIP_NAME) boot.dfu boot.frm pluto.dfu pluto.frm uboot-env.dfu
	@echo
	@echo "==> Release zip ready: $(FW_DIR)/build/$(RELEASE_ZIP_NAME)"

sysroot:                  ## Package the Buildroot sysroot tar.gz for this modified firmware build
	source $(VIVADO_SETTINGS) && $(MAKE) -C $(FW_DIR) sysroot
	@echo "==> Sysroot archive ready in $(FW_DIR)/build/"

certs:                    ## Generate a self-signed TLS cert/key (skipped if cert.pem/key.pem already exist)
	@if [ -f cert.pem ] && [ -f key.pem ]; then \
	  echo "certs: using existing cert.pem/key.pem (delete them to regenerate, or drop in your own)"; \
	else \
	  echo "certs: generating self-signed cert (CN=$(TARGET_IP))"; \
	  openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem \
	    -days 3650 -subj "/CN=$(TARGET_IP)" -addext "subjectAltName=IP:$(TARGET_IP)"; \
	fi

# =============================================================================
# Cleanup
# =============================================================================
.PHONY: clean distclean
clean:                    ## Remove build artifacts (firmware build/, Vivado project, cargo target); keeps patches
	-source $(VIVADO_SETTINGS) && $(MAKE) -C $(FW_DIR) clean
	-cargo clean

distclean: clean          ## clean + revert ALL patches, resetting the submodule trees to pristine ADI
	-bash scripts/revert.sh
	-for lvl in hdl linux buildroot u-boot-xlnx; do git -C $(FW_DIR)/$$lvl checkout . ; done
	-git -C $(FW_DIR) checkout .

# =============================================================================
# Rust host application
# =============================================================================
.PHONY: app app-cross deploy deploy-cross run run-cross debug local ssh app-diagnostics deploy-diagnostics run-diagnostics
local:                    ## Build+run the app natively on the host (needs host libiio)
	cargo run

app-diagnostics:          ## Cross-compile the diagnostics binary for the Pluto (armv7)
	cargo build --bin diagnostics --target $(TARGET_ARCH) --release

deploy-diagnostics: app-diagnostics ## Deploy the diagnostics binary to the Pluto device (kills live app first)
	ssh -t $(TARGET_USER)@$(TARGET_IP) "killall -q pluto || true; killall -q diagnostics || true;"
	scp target/$(TARGET_ARCH)/release/diagnostics $(TARGET_USER)@$(TARGET_IP):$(TARGET_DIR)

run-diagnostics: deploy-diagnostics ## Deploy + run the diagnostics on the Pluto device (e.g., ARGS="--test-spec-audio-sweep")
	ssh -t $(TARGET_USER)@$(TARGET_IP) "$(TARGET_DIR)diagnostics $(ARGS)"

app:                      ## Compile the app natively for the Pluto (armv7); needs host Linaro toolchain and buildroot
	cargo build --target $(TARGET_ARCH) --release

app-cross:                ## Cross-compile the app inside the Docker container
ifeq ($(OS),Windows_NT)
	cross-compile\make.bat app
else
	./cross-compile/make.sh app
endif

deploy: app certs         ## Native compile + copy binary, TLS certs, and static/ to the device
	ssh -t $(TARGET_USER)@$(TARGET_IP) "killall -q $(BINARY_NAME) || true;"
	scp target/$(TARGET_ARCH)/release/$(BINARY_NAME) $(TARGET_USER)@$(TARGET_IP):$(TARGET_DIR)
	scp cert.pem key.pem $(TARGET_USER)@$(TARGET_IP):$(TARGET_DIR)
	scp -r static $(TARGET_USER)@$(TARGET_IP):$(TARGET_DIR)
	@echo "Deployed to $(TARGET_DIR)$(BINARY_NAME) on $(TARGET_IP)"

deploy-cross: app-cross certs ## Docker cross-compile + copy binary, TLS certs, and static/ to the device
	ssh -t $(TARGET_USER)@$(TARGET_IP) "killall -q $(BINARY_NAME) || true;"
	scp target/$(TARGET_ARCH)/release/$(BINARY_NAME) $(TARGET_USER)@$(TARGET_IP):$(TARGET_DIR)
	scp cert.pem key.pem $(TARGET_USER)@$(TARGET_IP):$(TARGET_DIR)
	scp -r static $(TARGET_USER)@$(TARGET_IP):$(TARGET_DIR)
	@echo "Deployed to $(TARGET_DIR)$(BINARY_NAME) on $(TARGET_IP)"

run: deploy               ## Deploy + execute the app on the device (foreground)
	ssh -t $(TARGET_USER)@$(TARGET_IP) "$(TARGET_DIR)$(BINARY_NAME) $(ARGS)"

run-cross: deploy-cross   ## Docker cross-compile + deploy + execute on the device (foreground)
	ssh -t $(TARGET_USER)@$(TARGET_IP) "$(TARGET_DIR)$(BINARY_NAME) $(ARGS)"

debug: deploy             ## Deploy + run on the device with debug logging (RUST_LOG=debug)
	ssh -t $(TARGET_USER)@$(TARGET_IP) "RUST_LOG=debug $(TARGET_DIR)$(BINARY_NAME) $(ARGS)"

ssh:                      ## Reset the device's known_hosts entry and install your SSH key
	ssh-keygen -R "$(TARGET_IP)"
	ssh-copy-id root@$(TARGET_IP)

# =============================================================================
.PHONY: help
help:                     ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
