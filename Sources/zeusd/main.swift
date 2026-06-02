import ZeusDaemonKit
import ZeusProtocol
import Foundation

// zeusd — the Zeus background daemon. Owns PTYs, sessions, statuses.
// Sessions survive app quits because this process outlives the app.

// Stamp process start on stderr (captured to zeusd.boot.log by the app's
// launcher). This is our ONLY visibility for failures before DaemonLog exists —
// a silent singleton-lock exit, a dyld/exec problem, or an early crash. We use
// stderr here, not DaemonLog, because DaemonLog truncates its file on init: a
// racing loser daemon would otherwise wipe the winner's log.
FileHandle.standardError.write(
    Data("zeusd: process start pid=\(ProcessInfo.processInfo.processIdentifier)\n".utf8))

do {
    try ZeusPaths.ensureDirectoriesExist()
} catch {
    FileHandle.standardError.write(Data("zeusd: cannot create dirs: \(error)\n".utf8))
    exit(1)
}

// Singleton guard: hold an exclusive lock for our lifetime so a second daemon
// (e.g. spawned by a relaunching app whose probe raced) exits instead of
// unlinking + stealing the live daemon's socket and orphaning its PTYs.
// The fd is intentionally leaked (kept open until process exit).
let lockPath = ZeusPaths.appSupport.appendingPathComponent("daemon.lock").path
let lockFD = open(lockPath, O_CREAT | O_RDWR, 0o600)
if lockFD < 0 || flock(lockFD, LOCK_EX | LOCK_NB) != 0 {
    // Another daemon already owns it — nothing to do. Say so on stderr so a
    // "no zeusd.log" report is distinguishable from a never-launched daemon.
    FileHandle.standardError.write(
        Data("zeusd: another daemon owns the lock (fd=\(lockFD)) — exiting\n".utf8))
    exit(0)
}

let config = DaemonConfig.standard()
try? InjectionBuilder.writeClaudeHooksFile(into: ZeusPaths.injectDir)

// Self-install the `zeus` CLI into binDir so the stable $ZEUS_CLI path
// (referenced by injected Claude hooks and Codex notify) resolves whether we're
// running from an SPM build dir or an app bundle. The daemon and CLI always ship
// side by side.
installCLIHelper()
try? InjectionBuilder.writeClaudeMcpFile(
    into: ZeusPaths.injectDir, cliPath: config.cliPath)

func installCLIHelper() {
    let daemonDir = URL(fileURLWithPath: CommandLine.arguments[0])
        .resolvingSymlinksInPath()
        .deletingLastPathComponent()
    for name in ["zeus", "zeus-mcp"] {
        let source = daemonDir.appendingPathComponent(name)
        guard FileManager.default.isExecutableFile(atPath: source.path) else { continue }
        let dest = ZeusPaths.binDir.appendingPathComponent(name)
        if source.resolvingSymlinksInPath() == dest.resolvingSymlinksInPath() { continue }
        do {
            try? FileManager.default.removeItem(at: dest)
            try FileManager.default.copyItem(at: source, to: dest)
            DaemonLog.shared.log("installed helper: \(source.path) -> \(dest.path)")
        } catch {
            DaemonLog.shared.log("helper install failed for \(name): \(error)")
        }
    }
}

let daemon = Daemon(config: config)

// Graceful shutdown on SIGTERM/SIGINT: final state snapshot, then exit.
signal(SIGTERM, SIG_IGN)
signal(SIGINT, SIG_IGN)
let sigterm = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
let sigint = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
for source in [sigterm, sigint] {
    source.setEventHandler {
        Task {
            await daemon.shutdown()
            exit(0)
        }
    }
    source.activate()
}

Task {
    do {
        try await daemon.start()
        DaemonLog.shared.log(
            "zeusd \(Daemon.build) ready (pid \(ProcessInfo.processInfo.processIdentifier))")
    } catch {
        DaemonLog.shared.log("fatal: \(error)")
        exit(1)
    }
}

dispatchMain()
