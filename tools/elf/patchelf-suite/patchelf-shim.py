#!/usr/bin/env python3
"""Run `arwen elf` with patchelf's command line, to run patchelf's test suite
against arwen. The operations are applied in command-line order.

Not supported: --remove-section. --page-size is accepted and ignored: arwen
aligns new segments like the existing ones.
"""

import os
import shutil
import stat
import subprocess
import sys
import tempfile

ARWEN = os.environ["ARWEN"]
WITH_ARG = {
    "--set-interpreter",
    "--interpreter",
    "--page-size",
    "--set-rpath",
    "--add-rpath",
    "--set-soname",
    "--add-needed",
    "--remove-needed",
    "--allowed-rpath-prefixes",
    "--clear-symbol-version",
    "--set-os-abi",
    "--rename-dynamic-symbols",
    "--output",
}


def fail(msg):
    print(f"patchelf-shim: {msg}", file=sys.stderr)
    sys.exit(1)


def value(v):
    """patchelf reads `@file` arguments from the file."""
    if v.startswith("@"):
        try:
            return open(v[1:]).read()
        except OSError as e:
            fail(f"getting info about '{v[1:]}': {e.strerror}")
    return v


def parse(argv):
    ops, files, opts = [], [], {"force_rpath": False, "output": None, "prefixes": []}
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--version":
            print("patchelf (arwen shim)")
            sys.exit(0)
        if a in ("--debug", "--no-sort"):
            pass
        elif a == "--force-rpath":
            opts["force_rpath"] = True
        elif a == "--replace-needed":
            ops.append((a, argv[i + 1], argv[i + 2]))
            i += 2
        elif a in WITH_ARG:
            if i + 1 >= len(argv):
                fail(f"missing argument to {a}")
            v = value(argv[i + 1])
            if a == "--output":
                opts["output"] = v
            elif a == "--allowed-rpath-prefixes":
                opts["prefixes"] += [p for p in v.split(":") if p]
            elif a != "--page-size":
                ops.append((a, v))
            i += 1
        elif a.startswith("--"):
            if a == "--remove-section":
                fail("--remove-section is not supported")
            ops.append((a,))
        else:
            files.append(a)
        i += 1
    return ops, files, opts


def arwen(*args, quiet=False):
    r = subprocess.run(
        [ARWEN, "elf", *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    if not quiet:
        sys.stdout.write(r.stdout)
        sys.stdout.flush()
    if r.returncode != 0:
        sys.stderr.write(r.stderr)
        sys.exit(r.returncode)


def run(op, f, opts):
    name = op[0]
    simple = {
        "--print-interpreter": "print-interpreter",
        "--print-os-abi": "print-os-abi",
        "--print-soname": "print-soname",
        "--remove-rpath": "remove-rpath",
        "--print-rpath": "print-rpath",
        "--print-needed": "print-needed",
        "--no-default-lib": "no-default-lib",
        "--add-debug-tag": "add-debug-tag",
        "--print-execstack": "print-exec-stack",
        "--clear-execstack": "clear-exec-stack",
        "--set-execstack": "set-exec-stack",
    }
    if name in simple:
        arwen(simple[name], f)
    elif name in ("--set-interpreter", "--interpreter"):
        arwen("set-interpreter", op[1], f)
    elif name == "--set-rpath":
        arwen("set-rpath", op[1], f)
    elif name == "--add-rpath":
        arwen("add-rpath", op[1], f)
    elif name == "--shrink-rpath":
        arwen("shrink-rpath", f, *[x for p in opts["prefixes"] for x in ("-a", p)])
    elif name == "--set-soname":
        arwen("set-soname", op[1], f)
    elif name == "--add-needed":
        arwen("add-needed", f, op[1])
    elif name == "--remove-needed":
        arwen("remove-needed", f, op[1])
    elif name == "--replace-needed":
        arwen("replace-needed", f, f"{op[1]}={op[2]}")
    elif name == "--clear-symbol-version":
        arwen("clear-symbol-version", op[1], f)
    elif name == "--set-os-abi":
        arwen("set-os-abi", op[1], f)
    elif name == "--rename-dynamic-symbols":
        pairs = [line.split() for line in open(op[1]) if line.strip()]
        arwen("rename-dynamic-symbols", f, *[f"{a}={b}" for a, b in pairs])
    else:
        fail(f"unknown option {name}")
    if opts["force_rpath"] and name in ("--set-rpath", "--add-rpath", "--shrink-rpath"):
        arwen("force-rpath", f)


def main():
    ops, files, opts = parse(sys.argv[1:])
    if not files:
        fail("missing filename")
    if opts["output"] and len(files) != 1:
        fail("--output needs exactly one input file")
    for f in files:
        target = f
        if opts["output"]:
            # Edit a copy, so that a failed edit leaves the output as it was.
            fd, target = tempfile.mkstemp(prefix="patchelf-shim-")
            os.close(fd)
            shutil.copyfile(f, target)
            mode = stat.S_IMODE(os.stat(f).st_mode)
            os.chmod(target, mode | stat.S_IWUSR)
        try:
            if not ops:
                arwen("print-needed", target, quiet=True)  # parse only
            for op in ops:
                run(op, target, opts)
            if opts["output"]:
                if opts["output"] != "/dev/null":
                    shutil.move(target, opts["output"])
                    os.chmod(opts["output"], mode)
        finally:
            if opts["output"] and os.path.exists(target):
                os.remove(target)


main()
