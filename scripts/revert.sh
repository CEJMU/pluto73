#!/usr/bin/env bash
# Revert ONLY the custom patches applied by apply.sh (reverse git-apply).
# Pluto+'s own patches are left in place. To get back to a pristine tree,
# run `git checkout .` inside each submodule afterwards.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fw="$root/plutoplus/plutosdr-fw"

echo "Reverting custom patches:"
for lvl in hdl linux u-boot-xlnx buildroot; do
  echo "  - $lvl"
  git -C "$fw/$lvl" apply -R --whitespace=nowarn "$root/patches/$lvl.diff"
done
echo "Done."
