#include "cdirijor_pty.h"

#include <fcntl.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>
#include <util.h>

pid_t dirijor_pty_spawn(const char *path,
                        char *const argv[],
                        char *const envp[],
                        const char *cwd,
                        unsigned short cols,
                        unsigned short rows,
                        int *out_master) {
    int master = -1, slave = -1;
    struct winsize ws;
    ws.ws_row = rows;
    ws.ws_col = cols;
    ws.ws_xpixel = 0;
    ws.ws_ypixel = 0;

    if (openpty(&master, &slave, NULL, NULL, &ws) != 0) {
        return -1;
    }

    pid_t pid = fork();
    if (pid < 0) {
        close(master);
        close(slave);
        return -1;
    }

    if (pid == 0) {
        // Child: async-signal-safe syscalls only.
        close(master);

        // Reset signal state to a clean default. The daemon (libdispatch/Swift
        // runtime) blocks or ignores signals like SIGWINCH, and a child inherits
        // the signal mask and any SIG_IGN dispositions across execve — which
        // would make the agent (e.g. Claude Code) never see terminal resizes and
        // so never repaint. Unblock everything and restore default handlers.
        sigset_t empty;
        sigemptyset(&empty);
        sigprocmask(SIG_SETMASK, &empty, NULL);
        for (int sig = 1; sig < NSIG; sig++) {
            signal(sig, SIG_DFL);
        }

        setsid();                        // become session leader
        ioctl(slave, TIOCSCTTY, 0);      // slave becomes our controlling tty
        dup2(slave, 0);
        dup2(slave, 1);
        dup2(slave, 2);
        if (slave > 2) {
            close(slave);
        }
        // Close any other inherited descriptors (daemon socket, kqueue, …).
        int maxfd = getdtablesize();
        for (int fd = 3; fd < maxfd; fd++) {
            close(fd);
        }
        if (cwd != NULL) {
            (void)chdir(cwd);
        }
        execve(path, argv, envp);
        _exit(127);  // only reached if execve fails
    }

    // Parent.
    close(slave);
    *out_master = master;
    return pid;
}
