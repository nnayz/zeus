import Darwin
import Foundation

/// One process observed in a session's descendant tree. `startSec` is the
/// kernel's process start time (`pbi_start_tvsec`) — the pid-reuse guard: a
/// recycled pid gets a new start time, so signal only when it still matches.
struct ProcSample: Sendable, Hashable {
    var pid: pid_t
    var startSec: Int64
}

/// Enumerates and signals a session's full descendant process tree.
///
/// Pure Swift over libproc + sysctl (both surfaced by `import Darwin`):
/// - net (a): recursive `proc_listchildpids` — direct parent/child links.
/// - net (b): `sysctl KERN_PROC_PGRP` for every process group discovered —
///   catches double-forked servers that kept the group but lost the parent.
/// - net (c) (`KERN_PROC_TTY` by slave dev) is intentionally skipped: capturing
///   the slave `dev_t` needs a PTY-spawn signature change owned elsewhere, and
///   nets a+b cover processes that neither setsid'd nor re-parented away.
///
/// All calls are blocking syscalls; callers run them at coarse cadence
/// (hibernate/wake, 30s governor scans), never on a hot path.
enum ProcessTree {
    // MARK: Per-pid probes

    /// Process start time in seconds, or nil if the pid is gone/inaccessible.
    static func startTime(_ pid: pid_t) -> Int64? {
        var info = proc_taskallinfo()
        let size = Int32(MemoryLayout<proc_taskallinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTASKALLINFO, 0, &info, size) == size else { return nil }
        return Int64(info.pbsd.pbi_start_tvsec)
    }

    /// The process's executable name (proc_name, 2*MAXCOMLEN buffer), or nil.
    /// NOTE: this is the runtime-settable process TITLE — Claude Code retitles
    /// itself to its version string ("2.1.204"). Identity checks should use
    /// `execName` instead.
    static func processName(_ pid: pid_t) -> String? {
        var buffer = [CChar](repeating: 0, count: 64)
        let n = proc_name(pid, &buffer, UInt32(buffer.count))
        guard n > 0 else { return nil }
        return decode(buffer)
    }

    /// The full executable path of the process (proc_pidpath) — immune to
    /// setproctitle-style renames — or nil.
    static func execPath(_ pid: pid_t) -> String? {
        var buffer = [CChar](repeating: 0, count: 4096)  // PROC_PIDPATHINFO_MAXSIZE
        let n = proc_pidpath(pid, &buffer, UInt32(buffer.count))
        guard n > 0 else { return nil }
        let path = decode(buffer)
        return path.isEmpty ? nil : path
    }

    /// The process's live current working directory (`PROC_PIDVNODEPATHINFO`),
    /// or nil if the pid is gone/inaccessible. This is how we notice an agent
    /// that `chdir`'d itself — Claude Code's `--worktree`/EnterWorktree move the
    /// process into `.claude/worktrees/<name>/` without the daemon spawning it,
    /// so the recorded cwd goes stale.
    static func currentWorkingDir(_ pid: pid_t) -> String? {
        var info = proc_vnodepathinfo()
        let size = Int32(MemoryLayout<proc_vnodepathinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &info, size) == size else { return nil }
        let path = withUnsafePointer(to: &info.pvi_cdir.vip_path) {
            $0.withMemoryRebound(to: CChar.self, capacity: Int(MAXPATHLEN)) {
                String(
                    decodingCString: UnsafeRawPointer($0).assumingMemoryBound(to: UInt8.self),
                    as: UTF8.self)
            }
        }
        return path.isEmpty ? nil : path
    }

    private static func decode(_ buffer: [CChar]) -> String {
        String(
            decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) },
            as: UTF8.self)
    }

    /// BSD process status (`SSTOP` == stopped), or nil if the pid is gone.
    static func status(_ pid: pid_t) -> UInt32? {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
        return info.pbi_status
    }

    /// Direct children of `pid` via proc_listchildpids.
    ///
    /// Return-value semantics (verified empirically — they differ from the
    /// proc_listpids byte convention): the NULL probe returns an upper-bound
    /// COUNT, and the fill call returns the NUMBER OF PIDS written, not bytes.
    /// Dividing by stride here would truncate small families to zero and
    /// silently miss children (e.g. a shell's backgrounded dev server).
    static func childPids(_ pid: pid_t) -> [pid_t] {
        let stride = MemoryLayout<pid_t>.stride
        let probe = proc_listchildpids(pid, nil, 0)
        guard probe > 0 else { return [] }
        // Headroom: children can appear between the probe and the fill.
        var buffer = [pid_t](repeating: 0, count: Int(probe) + 8)
        let filled = proc_listchildpids(pid, &buffer, Int32(buffer.count * stride))
        guard filled > 0 else { return [] }
        return Array(buffer.prefix(Int(filled))).filter { $0 > 0 }
    }

    /// All pids in process group `pgid` via sysctl KERN_PROC_PGRP.
    static func pgrpPids(_ pgid: pid_t) -> [pid_t] {
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PGRP, pgid]
        var size = 0
        guard sysctl(&mib, UInt32(mib.count), nil, &size, nil, 0) == 0, size > 0 else { return [] }
        let stride = MemoryLayout<kinfo_proc>.stride
        // Headroom for processes forked between the two sysctl calls.
        var buffer = [kinfo_proc](repeating: kinfo_proc(), count: size / stride + 8)
        size = buffer.count * stride
        guard sysctl(&mib, UInt32(mib.count), &buffer, &size, nil, 0) == 0 else { return [] }
        return buffer.prefix(size / stride).map { $0.kp_proc.p_pid }.filter { $0 > 0 }
    }

    // MARK: Enumeration

    /// The descendant tree rooted at `root`: fixed-point closure over direct
    /// children + every process group discovered along the way. `slaveDev` is
    /// reserved for the (skipped) controlling-tty net and currently unused.
    static func enumerate(root: pid_t, slaveDev: dev_t? = nil) -> [ProcSample] {
        _ = slaveDev
        guard root > 1 else { return [] }
        var seen: Set<pid_t> = []
        var frontier: [pid_t] = [root]
        var groupsScanned: Set<pid_t> = []
        let selfPid = getpid()

        while let pid = frontier.popLast() {
            guard pid > 1, pid != selfPid, !seen.contains(pid) else { continue }
            seen.insert(pid)
            frontier.append(contentsOf: childPids(pid))
            if let pgid = groupID(of: pid), pgid > 1, !groupsScanned.contains(pgid) {
                groupsScanned.insert(pgid)
                frontier.append(contentsOf: pgrpPids(pgid))
            }
        }
        return seen.compactMap { pid in
            startTime(pid).map { ProcSample(pid: pid, startSec: $0) }
        }
    }

    private static func groupID(of pid: pid_t) -> pid_t? {
        let pgid = getpgid(pid)
        return pgid > 0 ? pgid : nil
    }

    // MARK: Signalling

    /// SIGSTOPs the whole tree top-down (root first, so it can't spawn more),
    /// then re-enumerates and re-stops to a fixed point: every discovered pid
    /// verified `SSTOP`. Stopped processes can't fork, so this converges.
    /// Returns the final tree (with start times) for later CONT/KILL.
    static func stopAll(root: pid_t) -> [ProcSample] {
        guard root > 1 else { return [] }
        Darwin.kill(root, SIGSTOP)

        var samples: [ProcSample] = []
        for _ in 0..<6 {
            samples = enumerate(root: root)
            var allStopped = true
            for sample in samples {
                if status(sample.pid) != UInt32(SSTOP) {
                    Darwin.kill(sample.pid, SIGSTOP)
                    allStopped = false
                }
            }
            if allStopped { break }
        }
        // Keep only pids still verifiably ours (start time intact).
        return samples.filter { startTime($0.pid) == $0.startSec }
    }

    /// SIGCONTs the tree bottom-up (leaves first: a resumed parent never
    /// observes a stopped child). Children start after their parents, so
    /// descending start time is a valid leaves-first order. Start-time verified.
    static func continueAll(_ samples: [ProcSample]) {
        for sample in samples.sorted(by: { $0.startSec > $1.startSec }) {
            guard startTime(sample.pid) == sample.startSec else { continue }
            Darwin.kill(sample.pid, SIGCONT)
        }
    }

    /// SIGKILLs every sample whose pid still has the recorded start time
    /// (never signals a recycled pid).
    static func killAll(_ samples: [ProcSample]) {
        for sample in samples {
            guard sample.pid > 1, startTime(sample.pid) == sample.startSec else { continue }
            Darwin.kill(sample.pid, SIGKILL)
            // A stopped process only reaps after CONT delivers the pending KILL.
            Darwin.kill(sample.pid, SIGCONT)
        }
    }

    // MARK: Memory

    /// Total physical footprint of `pids` (Σ ri_phys_footprint — the number
    /// Activity Monitor shows; RSS undercounts on Apple Silicon).
    static func footprint(_ pids: [pid_t]) -> UInt64 {
        var total: UInt64 = 0
        for pid in pids {
            var info = rusage_info_v4()
            let rc = withUnsafeMutablePointer(to: &info) { pointer in
                pointer.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { reboundPointer in
                    proc_pid_rusage(pid, RUSAGE_INFO_V4, reboundPointer)
                }
            }
            if rc == 0 { total &+= info.ri_phys_footprint }
        }
        return total
    }
}
