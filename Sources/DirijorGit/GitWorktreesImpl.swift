import DirijorCore
import Foundation

// Real implementations backing the frozen public API in GitWorktrees.swift.

extension GitWorktrees {
    /// Runs `/usr/bin/git` with `args` in `directory`, capturing stdout+stderr.
    /// Trims surrounding whitespace/newlines. Non-zero exit throws GitError with
    /// the captured stderr (falling back to stdout when stderr is empty).
    static func runImpl(_ args: [String], in directory: String) throws -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        process.arguments = args
        process.currentDirectoryURL = URL(fileURLWithPath: directory)

        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        do {
            try process.run()
        } catch {
            throw GitError(message: "failed to launch git: \(error.localizedDescription)", exitCode: -1)
        }

        // Read both pipes fully before waiting to avoid deadlock on large output.
        let stdoutData = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
        let stderrData = stderrPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        let stdout = String(decoding: stdoutData, as: UTF8.self)
        let stderr = String(decoding: stderrData, as: UTF8.self)

        if process.terminationStatus != 0 {
            let message = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            let fallback = stdout.trimmingCharacters(in: .whitespacesAndNewlines)
            throw GitError(
                message: message.isEmpty ? fallback : message,
                exitCode: process.terminationStatus
            )
        }

        return stdout.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Parses `git worktree list --porcelain` output into WorktreeInfo entries.
    /// Blocks are separated by blank lines. Recognized keys per block:
    ///   worktree <path> | HEAD <sha> | branch refs/heads/<name> | bare | detached | prunable
    static func parsePorcelainListImpl(_ porcelain: String) -> [WorktreeInfo] {
        var results: [WorktreeInfo] = []

        var currentPath: String?
        var currentBranch: String?
        var isBare = false
        var isDetached = false
        var isPrunable = false

        func flush() {
            guard let path = currentPath else { return }
            results.append(
                WorktreeInfo(
                    path: path,
                    branch: currentBranch,
                    isBare: isBare,
                    isDetached: isDetached,
                    isPrunable: isPrunable
                )
            )
            currentPath = nil
            currentBranch = nil
            isBare = false
            isDetached = false
            isPrunable = false
        }

        for rawLine in porcelain.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(rawLine)
            if line.isEmpty {
                // Blank line terminates a block.
                flush()
                continue
            }
            if line.hasPrefix("worktree ") {
                // A new worktree line begins a new block; flush any in progress.
                flush()
                currentPath = String(line.dropFirst("worktree ".count))
            } else if line.hasPrefix("branch ") {
                var ref = String(line.dropFirst("branch ".count))
                if ref.hasPrefix("refs/heads/") {
                    ref = String(ref.dropFirst("refs/heads/".count))
                }
                currentBranch = ref
            } else if line == "bare" {
                isBare = true
            } else if line == "detached" {
                isDetached = true
            } else if line == "prunable" || line.hasPrefix("prunable ") {
                isPrunable = true
            }
            // HEAD <sha> and other keys are ignored for WorktreeInfo.
        }
        flush()

        return results
    }

    private static let adjectives = [
        "brisk", "calm", "deft", "eager", "fleet", "gentle", "hardy", "keen",
        "lively", "merry", "nimble", "plucky", "quiet", "rapid", "steady",
        "swift", "tidy", "vivid", "witty", "zesty",
    ]

    private static let nouns = [
        "otter", "heron", "maple", "cedar", "falcon", "willow", "badger", "sparrow",
        "cypress", "marten", "juniper", "raven", "birch", "lynx", "hazel",
        "osprey", "aspen", "finch", "poplar", "wren",
    ]

    /// "dirijor/<adjective>-<noun>-<4hex>".
    static func generatedBranchNameImpl() -> String {
        let adjective = adjectives.randomElement() ?? "brisk"
        let noun = nouns.randomElement() ?? "otter"
        let hex = String(format: "%04x", UInt16.random(in: 0...0xFFFF))
        return "dirijor/\(adjective)-\(noun)-\(hex)"
    }

    /// Lowercases and turns any run of characters outside [a-z0-9-] into a single
    /// dash, then trims leading/trailing dashes.
    static func branchToPathSlugImpl(_ branch: String) -> String {
        let lowered = branch.lowercased()
        var slug = ""
        var lastWasDash = false
        for scalar in lowered.unicodeScalars {
            let isAllowed = (scalar >= "a" && scalar <= "z")
                || (scalar >= "0" && scalar <= "9")
            if isAllowed {
                slug.unicodeScalars.append(scalar)
                lastWasDash = false
            } else {
                // "/", spaces, and anything else collapse to a single dash.
                if !lastWasDash {
                    slug.append("-")
                    lastWasDash = true
                }
            }
        }
        // Trim leading/trailing dashes.
        while slug.hasPrefix("-") { slug.removeFirst() }
        while slug.hasSuffix("-") { slug.removeLast() }
        return slug
    }
}
