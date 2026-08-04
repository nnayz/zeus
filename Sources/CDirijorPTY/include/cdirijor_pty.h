#ifndef CDIRIJOR_PTY_H
#define CDIRIJOR_PTY_H

#include <sys/types.h>

/// Opens a PTY and forks a child that runs `path` with `argv`/`envp` in a new
/// session whose controlling terminal is the PTY slave. On success returns the
/// child pid and writes the master fd to *out_master; returns -1 on failure.
///
/// The controlling-tty setup (setsid + TIOCSCTTY) is why this is fork/exec in C
/// rather than posix_spawn: strict shells (fish) require a real controlling
/// terminal or they abort job-control setup and exit.
pid_t dirijor_pty_spawn(const char *path,
                        char *const argv[],
                        char *const envp[],
                        const char *cwd,
                        unsigned short cols,
                        unsigned short rows,
                        int *out_master);

#endif
