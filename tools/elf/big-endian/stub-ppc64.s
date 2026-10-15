# Stand-in for libc.so.6: versioned setlocale and dlopen that do nothing.
	.abiversion 2
	.text
	.globl setlocale
	.type setlocale,@function
setlocale:
	blr
	.size setlocale,.-setlocale
	.globl dlopen
	.type dlopen,@function
dlopen:
	blr
	.size dlopen,.-dlopen
