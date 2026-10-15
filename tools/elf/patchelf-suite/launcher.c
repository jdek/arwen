/* An ELF executable that runs the patchelf shim, for tests that inspect
 * the patchelf binary itself. */
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **argv) {
    char **args = calloc(argc + 2, sizeof(char *));
    args[0] = "python3";
    args[1] = SHIM;
    for (int i = 1; i < argc; i++)
        args[i + 1] = argv[i];
    execvp("python3", args);
    perror("python3");
    return 127;
}
