# Source of tests/data/elf-big-endian/mips/lib/libfoo.so, a big-endian 32-bit counterpart of
# tools/elf/libfoo.c: it calls setlocale and dlopen through the GOT and refers to
# setlocale from RELRO data.
	.abicalls
	.text
	.globl foo_locale
	.type foo_locale,@function
	.ent foo_locale
foo_locale:
	.set noreorder
	.cpload $25
	.set reorder
	lw $25,%call16(setlocale)($28)
	jr $25
	.end foo_locale
	.globl foo_open
	.type foo_open,@function
	.ent foo_open
foo_open:
	.set noreorder
	.cpload $25
	.set reorder
	lw $25,%call16(dlopen)($28)
	jr $25
	.end foo_open
	.section .data.rel.ro,"aw"
	.align 2
	.globl foo_table
	.type foo_table,@object
foo_table:
	.word foo_locale
	.word setlocale
	.size foo_table,8
