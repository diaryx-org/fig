/* ABI probe (C). Compiled by `cargo xtask abi-check` against the C ABI static
 * library to prove fig.h is valid, self-contained C that links. The
 * twin abi_probe.cpp does the same under a C++ compiler (the header guards
 * `extern "C"`). Symbol presence is checked separately, against libfig.a's exports. */
#include "fig.h"

int main(void) {
    return 0;
}
