# Stand-in for libc.so.6: versioned setlocale and dlopen that do nothing.
	.abicalls
	.text
	.globl setlocale
	.type setlocale,@function
	.ent setlocale
setlocale:
	jr $ra
	nop
	.end setlocale
	.globl dlopen
	.type dlopen,@function
	.ent dlopen
dlopen:
	jr $ra
	nop
	.end dlopen
