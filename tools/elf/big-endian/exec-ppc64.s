# Source of tests/data/elf-big-endian/ppc64/exec/hello, a big-endian 64-bit executable with
# an interpreter that calls setlocale. It is only patched, never run.
	.abiversion 2
	.text
	.globl _start
	.type _start,@function
_start:
0:	addis 2,12,.TOC.-0b@ha
	addi 2,2,.TOC.-0b@l
	.localentry _start,.-_start
	bl setlocale
	nop
	b .
	.size _start,.-_start
