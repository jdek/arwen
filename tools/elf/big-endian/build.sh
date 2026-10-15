#!/bin/sh
# Build the big-endian fixtures in tests/data/elf-big-endian/{ppc64,mips}/ with cross binutils
# (no compiler or C library needed), e.g. from nixpkgs:
#   PPC64=$(nix build --print-out-paths nixpkgs#pkgsCross.ppc64.buildPackages.binutils-unwrapped)/bin/powerpc64-unknown-linux-gnuabielfv2-
#   MIPS=$(nix build --print-out-paths nixpkgs#pkgsCross.mips-linux-gnu.buildPackages.binutils-unwrapped)/bin/mips-unknown-linux-gnu-
#   PPC64=$PPC64 MIPS=$MIPS tools/elf/big-endian/build.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
data=$here/../../../tests/data/elf-big-endian
work=$(mktemp -d)
trap 'rm -r "$work"' EXIT

build() { # arch prefix interpreter
    arch=$1 tool=$2 interp=$3
    cd "$work"
    mkdir "$arch" && cd "$arch"
    "${tool}as" -o stub.o "$here/stub-$arch.s"
    "${tool}ld" -shared -soname libc.so.6 --version-script "$here/stub.map" -o libc.so.6 stub.o
    # libdl.so.2 is only needed by name; it exports nothing, so that setlocale
    # and dlopen bind to the versioned definitions in libc.so.6.
    echo '{ local: *; };' > empty.map
    "${tool}ld" -shared -soname libdl.so.2 --version-script empty.map -o libdl.so.2 stub.o
    "${tool}as" -o lib.o "$here/lib-$arch.s"
    mkdir -p "$data/$arch/lib" "$data/$arch/exec"
    "${tool}ld" -shared -soname libfoo.so.1 -o "$data/$arch/lib/libfoo.so" lib.o \
        --no-as-needed libdl.so.2 libc.so.6 -z relro -z now -z noexecstack \
        -rpath /opt/foo/lib --enable-new-dtags -s
    "${tool}as" -o exec.o "$here/exec-$arch.s"
    "${tool}ld" -o "$data/$arch/exec/hello" exec.o libc.so.6 -dynamic-linker "$interp" \
        -z noexecstack -rpath /opt/hello/lib -s
}

build ppc64 "$PPC64" /lib64/ld64.so.2
build mips "$MIPS" /lib/ld.so.1
