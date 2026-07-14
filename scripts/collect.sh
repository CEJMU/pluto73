#!/usr/bin/env bash
# Regenerate patches/*.diff from the current submodule working trees, isolating
# OUR changes from Pluto+'s.
#
# The working tree of each nested submodule is:
#     clean ADI (pinned commit)  +  Pluto+'s patches  +  our patches
# all as uncommitted edits (Pluto+ never commits its patches). To recover only
# *our* delta we build a reference tree that is "clean ADI + Pluto+'s patches"
# in a throwaway git worktree, then diff the live tree against it. This never
# touches the live working tree.
#
# Run this after hand-editing tracked firmware files (device trees, the
# system_bd.tcl wrapper, constraints, ...). To refresh the BLOCK DESIGN instead,
# re-run `write_bd_tcl hdl_bd/system_bd_design.tcl` in Vivado and place it at hdl_bd/
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fw="$root/plutoplus/plutosdr-fw"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

for lvl in hdl linux u-boot-xlnx buildroot; do
  sub="$fw/$lvl"
  ref="$tmp/$lvl"
  git -C "$sub" worktree add --detach "$ref" HEAD >/dev/null 2>&1
  # reference = clean ADI + Pluto+'s patch for this level
  git -C "$ref" apply --whitespace=nowarn "$root/plutoplus/patches/$lvl.diff"
  git -C "$ref" add -A >/dev/null 2>&1
  # overlay our current versions of every changed file
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    mkdir -p "$ref/$(dirname "$f")"
    cp "$sub/$f" "$ref/$f"
  done < <(git -C "$sub" diff --name-only)
  git -C "$ref" diff > "$root/patches/$lvl.diff"
  git -C "$sub" worktree remove --force "$ref" >/dev/null 2>&1
  echo "  regenerated patches/$lvl.diff ($(wc -l < "$root/patches/$lvl.diff") lines)"
done
echo "Done."
