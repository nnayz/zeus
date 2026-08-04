import DirijorProtocol
import Foundation

/// Session-scoped Git diff loading for daemon clients. The response stays
/// below the 4 MB NDJSON ceiling after base64 encoding.
enum WorktreeDiffLoader {
    static let maxPatchBytes = 2 * 1024 * 1024
    private static let commandTimeout: Duration = .seconds(30)

    static func load(
        cwd: String, host: HostEntry?, base: SessionDiffBase = .defaultBranch,
        runner: ShellRunner
    ) async throws -> SessionReadDiffResult {
        let shell = HostShell(runner: runner, host: host)
        let command = makeCommand(cwd: cwd, base: base)
        let result: ShellResult
        do {
            result = try await shell.run(command, timeout: commandTimeout)
        } catch {
            throw ControlError.internalError(
                "Git diff failed on \(shell.describesHost): \(error)")
        }
        guard result.ok else {
            let detail = result.stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            throw ControlError.badRequest(
                detail.isEmpty
                    ? "session cwd is not inside a Git repository on \(shell.describesHost)"
                    : "Git could not load changes on \(shell.describesHost): \(detail)")
        }

        guard let rootSeparator = result.stdout.firstIndex(of: "\0") else {
            throw ControlError.internalError(
                "Git diff on \(shell.describesHost) returned an invalid response")
        }
        let repoRoot = String(result.stdout[..<rootSeparator])
        let baseStart = result.stdout.index(after: rootSeparator)
        guard let baseSeparator = result.stdout[baseStart...].firstIndex(of: "\0") else {
            throw ControlError.internalError(
                "Git diff on \(shell.describesHost) returned an invalid response")
        }
        let baseRef = String(result.stdout[baseStart..<baseSeparator])
        let patchStart = result.stdout.index(after: baseSeparator)
        let bytes = Data(result.stdout[patchStart...].utf8)
        let truncated = bytes.count > maxPatchBytes
        return SessionReadDiffResult(
            patch: Data(bytes.prefix(maxPatchBytes)),
            repoRoot: repoRoot,
            truncated: truncated,
            baseRef: baseRef)
    }

    /// One SSH round trip: validate the cwd, prefix the absolute repo root,
    /// then stream tracked, staged, and untracked changes through a hard cap.
    /// `xargs -0` keeps spaces and newlines in untracked filenames intact.
    static func makeCommand(
        cwd: String, base: SessionDiffBase = .defaultBranch
    ) -> String {
        let quotedCwd = InjectionBuilder.shellQuotePath(cwd)
        let byteLimit = maxPatchBytes + 1
        let comparison = base.rawValue
        return """
            set -e
            cd \(quotedCwd)
            root=$(git rev-parse --show-toplevel)
            cd "$root"
            git status --porcelain=v1 -uno >/dev/null
            base_ref=HEAD
            base_commit=
            if git rev-parse --verify HEAD >/dev/null 2>&1; then
              if [ "\(comparison)" = head ]; then
                base_commit=HEAD
              else
                origin_head=$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null || true)
                base_ref=
                for candidate in "$origin_head" origin/main main origin/master master; do
                  [ -n "$candidate" ] || continue
                  if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
                    base_ref="$candidate"
                    break
                  fi
                done
                if [ -n "$base_ref" ]; then
                  base_commit=$(git merge-base "$base_ref" HEAD 2>/dev/null || true)
                fi
                if [ -z "$base_commit" ]; then
                  base_ref=HEAD
                  base_commit=HEAD
                fi
              fi
            fi
            printf '%s\\0%s\\0' "$root" "$base_ref"
            (
              if [ -n "$base_commit" ]; then
                git diff --no-ext-diff --no-color --unified=3 "$base_commit" --
              else
                git diff --no-ext-diff --no-color --unified=3 --cached --
                git diff --no-ext-diff --no-color --unified=3 --
              fi
              git ls-files --others --exclude-standard -z | \
                xargs -0 -n 1 sh -c '
                  [ "$#" -eq 0 ] && exit 0
                  git diff --no-index --no-color --unified=3 -- /dev/null "$1"
                  code=$?
                  [ "$code" -eq 0 ] || [ "$code" -eq 1 ]
                ' sh
            ) | head -c \(byteLimit)
            """
    }
}
