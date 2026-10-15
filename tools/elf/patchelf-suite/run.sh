#!/bin/sh
# Run patchelf's test suite against arwen, through a shim that translates
# patchelf's command line to `arwen elf`. The suite is fetched at a pinned
# commit and is not part of this repository.
#
# Usage: tools/elf/patchelf-suite/run.sh [ARWEN]
#   ARWEN defaults to target/release/arwen.
# Needs Linux, curl, autoconf, automake, make, gcc, g++, binutils and python3.
#
# Expected failures:
#   invalid-elf    checks patchelf's error messages; arwen rejects the same
#                  files with its own.
#   shared-rpath   expects a run path that is no longer used to be overwritten
#                  with 'X's; arwen leaves the unused string as it is.
set -eu

REV=b49de1b3384e7928bf0df9a889fe5a4e7b3fbddf
SHA256=8ed5382f1ac423328b1d50b2c89d96dbab156de0285d003ff51940975db66572
EXPECTED="invalid-elf shared-rpath"

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
ARWEN=$(realpath "${1:-$ROOT/target/release/arwen}")
WORK=$ROOT/target/patchelf-suite
SRC=$WORK/patchelf-$REV

# Nix's compiler wrapper adds a run path to everything it links, which the
# tests do not expect.
export NIX_DONT_SET_RPATH=1 "NIX_DONT_SET_RPATH_$(uname -m)_unknown_linux_gnu=1"

if [ ! -f "$SRC/tests/Makefile" ]; then
    mkdir -p "$WORK"
    curl -sSfL -o "$WORK/patchelf.tar.gz" "https://github.com/NixOS/patchelf/archive/$REV.tar.gz"
    echo "$SHA256  $WORK/patchelf.tar.gz" | sha256sum -c --quiet
    rm -rf "$SRC"
    tar -xzf "$WORK/patchelf.tar.gz" -C "$WORK"
    (cd "$SRC" && ./bootstrap.sh >/dev/null 2>&1 && ./configure -q)
fi

# Some tests read the interpreter of `../src/patchelf`, so it has to be an ELF
# executable rather than the script itself.
mkdir -p "$SRC/src"
gcc -O2 -DSHIM="\"$HERE/patchelf-shim.py\"" -o "$SRC/src/patchelf" "$HERE/launcher.c"

cd "$SRC/tests"
ARWEN=$ARWEN make -s -j"$(nproc)" check >"$WORK/check.log" 2>&1 || true
grep -E '^# (TOTAL|PASS|SKIP|FAIL|ERROR)' "$WORK/check.log"

status=0
for trs in *.trs; do
    grep -qE '^:test-result: (FAIL|ERROR)' "$trs" || continue
    name=${trs%.sh.trs}
    case " $EXPECTED " in
    *" $name "*) echo "expected failure: $name" ;;
    *) echo "FAIL: $name (see $SRC/tests/$name.sh.log)"; status=1 ;;
    esac
done
exit $status
