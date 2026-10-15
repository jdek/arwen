# Source of tests/data/elf-big-endian/mips/exec/hello, a big-endian 32-bit executable with
# an interpreter that calls setlocale. It is only patched, never run.
	.abicalls
	.text
	.globl __start
	.type __start,@function
	.ent __start
__start:
	.set noreorder
	.cpload $25
	.set reorder
	lw $25,%call16(setlocale)($28)
	jalr $25
1:	b 1b
	.end __start
