# Source of tests/data/elf-big-endian/ppc64/lib/libfoo.so, a big-endian 64-bit counterpart of
# tools/elf/libfoo.c: it calls setlocale and dlopen through the PLT and refers to
# setlocale from RELRO data.
	.abiversion 2
	.text
	.globl foo_locale
	.type foo_locale,@function
foo_locale:
0:	addis 2,12,.TOC.-0b@ha
	addi 2,2,.TOC.-0b@l
	.localentry foo_locale,.-foo_locale
	mflr 0
	std 0,16(1)
	stdu 1,-32(1)
	bl setlocale
	nop
	addi 1,1,32
	ld 0,16(1)
	mtlr 0
	blr
	.size foo_locale,.-foo_locale
	.globl foo_open
	.type foo_open,@function
foo_open:
0:	addis 2,12,.TOC.-0b@ha
	addi 2,2,.TOC.-0b@l
	.localentry foo_open,.-foo_open
	mflr 0
	std 0,16(1)
	stdu 1,-32(1)
	bl dlopen
	nop
	addi 1,1,32
	ld 0,16(1)
	mtlr 0
	blr
	.size foo_open,.-foo_open
	.section .data.rel.ro,"aw"
	.align 3
	.globl foo_table
	.type foo_table,@object
foo_table:
	.quad foo_locale
	.quad setlocale
	.size foo_table,16
