import Darwin
import Foundation

enum HolderProcessTree {
    static func startTime(_ pid: pid_t) -> Int64? {
        var info = proc_taskallinfo()
        let size = Int32(MemoryLayout<proc_taskallinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTASKALLINFO, 0, &info, size) == size else { return nil }
        return Int64(info.pbsd.pbi_start_tvsec)
    }

    static func status(_ pid: pid_t) -> UInt32? {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
        return info.pbi_status
    }

    static func childPids(_ pid: pid_t) -> [pid_t] {
        let stride = MemoryLayout<pid_t>.stride
        let count = proc_listchildpids(pid, nil, 0)
        guard count > 0 else { return [] }
        var buffer = [pid_t](repeating: 0, count: Int(count) + 8)
        let filled = proc_listchildpids(pid, &buffer, Int32(buffer.count * stride))
        guard filled > 0 else { return [] }
        return Array(buffer.prefix(Int(filled))).filter { $0 > 1 }
    }

    static func processGroupPids(_ group: pid_t) -> [pid_t] {
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PGRP, group]
        var size = 0
        guard sysctl(&mib, UInt32(mib.count), nil, &size, nil, 0) == 0, size > 0 else {
            return []
        }
        let stride = MemoryLayout<kinfo_proc>.stride
        var buffer = [kinfo_proc](repeating: kinfo_proc(), count: size / stride + 8)
        size = buffer.count * stride
        guard sysctl(&mib, UInt32(mib.count), &buffer, &size, nil, 0) == 0 else { return [] }
        return buffer.prefix(size / stride).map(\.kp_proc.p_pid).filter { $0 > 1 }
    }

    static func enumerate(root: pid_t) -> [HolderProcessSample] {
        guard root > 1 else { return [] }
        var seen: Set<pid_t> = []
        var frontier = [root]
        var scannedGroups: Set<pid_t> = []
        let holderPID = getpid()

        while let pid = frontier.popLast() {
            guard pid > 1, pid != holderPID, seen.insert(pid).inserted else { continue }
            frontier.append(contentsOf: childPids(pid))
            let group = getpgid(pid)
            if group > 1, scannedGroups.insert(group).inserted {
                frontier.append(contentsOf: processGroupPids(group))
            }
        }
        return seen.compactMap { pid in
            startTime(pid).map { HolderProcessSample(pid: pid, startSec: $0) }
        }
    }

    static func signal(root: pid_t, signal: Int32) -> [HolderProcessSample] {
        if signal == SIGSTOP {
            Darwin.kill(root, SIGSTOP)
            var tree: [HolderProcessSample] = []
            for _ in 0..<6 {
                tree = enumerate(root: root)
                var allStopped = true
                for sample in tree where status(sample.pid) != UInt32(SSTOP) {
                    Darwin.kill(sample.pid, SIGSTOP)
                    allStopped = false
                }
                if allStopped { break }
            }
            return tree.filter { startTime($0.pid) == $0.startSec }
        }

        let tree = enumerate(root: root)
        let ordered =
            signal == SIGCONT
            ? tree.sorted(by: { $0.startSec > $1.startSec })
            : tree
        signalGroup(root, signal)
        for sample in ordered where startTime(sample.pid) == sample.startSec {
            Darwin.kill(sample.pid, signal)
        }
        return tree
    }

    static func killTree(root: pid_t) {
        var tree = enumerate(root: root)
        _ = signal(root: root, signal: SIGTERM)
        _ = signal(root: root, signal: SIGCONT)

        let deadline = Date().addingTimeInterval(0.5)
        while Date() < deadline {
            if tree.allSatisfy({ startTime($0.pid) != $0.startSec }) { return }
            usleep(20_000)
        }
        tree.append(contentsOf: enumerate(root: root))
        signalGroup(root, SIGKILL)
        for sample in Set(tree) where startTime(sample.pid) == sample.startSec {
            Darwin.kill(sample.pid, SIGKILL)
            Darwin.kill(sample.pid, SIGCONT)
        }
    }

    private static func signalGroup(_ root: pid_t, _ signal: Int32) {
        if Darwin.kill(-root, signal) != 0, errno == ESRCH {
            Darwin.kill(root, signal)
        }
    }
}
