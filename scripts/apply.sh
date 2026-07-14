#!/usr/bin/env bash
# Apply the custom SSB-transceiver firmware/FPGA modifications ON TOP of the
# Pluto+ baseline.
#
# Prerequisite ordering (see README "Building"):
#   1. git submodule update --init --recursive
#   2. apply Pluto+'s own patches:  (cd plutoplus && scripts/apply.sh)
#   3. THIS script                                 <-- you are here
#
# Our patches modify tracked files in the nested plutosdr-fw submodules. The
# bulky/custom artifacts (the block-design export and the custom Verilog) are
# NOT patched into the submodule. They live in this repo (hdl_bd/, hdl_modules/)
# and are pulled in at build time by the wrapper projects/pluto/system_bd.tcl.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fw="$root/plutoplus/plutosdr-fw"

if [[ ! -d "$fw/hdl/projects/pluto" ]]; then
  echo "error: $fw/hdl not found -- did you init submodules and apply Pluto+'s patches first?" >&2
  exit 1
fi

echo "Applying custom patches on top of the Pluto+ baseline:"
for lvl in hdl linux u-boot-xlnx buildroot; do
  # idempotent: if the patch already reverse-applies cleanly, it's in place
  if git -C "$fw/$lvl" apply -R --check "$root/patches/$lvl.diff" >/dev/null 2>&1; then
    echo "  - $lvl (already applied, skipping)"
  else
    echo "  - $lvl"
    git -C "$fw/$lvl" apply --whitespace=nowarn "$root/patches/$lvl.diff"
  fi
done

echo
echo "Done. Block design will be sourced from hdl_bd/system_bd_design.tcl,"
echo "with hdl_modules/*.v registered by the wrapper projects/pluto/system_bd.tcl."
