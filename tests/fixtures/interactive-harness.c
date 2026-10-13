/* Deterministic external CLI boundary for portable-handoff transport tests. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <termios.h>
#include <unistd.h>

int main(void) {
    const char *directory = getenv("HERDR_TEST_CAPTURE_DIR");
    if (!directory) return 2;
    char path[4096];
    snprintf(path, sizeof(path), "%s/%ld.input", directory, (long)getpid());
    FILE *capture = fopen(path, "ab");
    if (!capture) return 3;
    struct termios tty;
    if (tcgetattr(STDIN_FILENO, &tty) == 0) {
        cfmakeraw(&tty);
        tcsetattr(STDIN_FILENO, TCSANOW, &tty);
    }
    printf("\033[?2004hPORTABLE_TEST_READY transcript-pid=%ld\r\n", (long)getpid());
    fflush(stdout);
    unsigned char bytes[4096];
    ssize_t length;
    while ((length = read(STDIN_FILENO, bytes, sizeof(bytes))) > 0) {
        fwrite(bytes, 1, (size_t)length, capture);
        fflush(capture);
    }
    fclose(capture);
    return 0;
}
