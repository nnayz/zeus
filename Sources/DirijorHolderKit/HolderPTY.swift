import CDirijorPTY
import Darwin
import Foundation

enum HolderPTY {
    static func spawn(_ spec: HolderLaunchSpec) throws -> (master: Int32, child: pid_t) {
        guard let executable = spec.argv.first else {
            throw HolderError.launch("argv is empty")
        }
        let argv: [UnsafeMutablePointer<CChar>?] = spec.argv.map { strdup($0) } + [nil]
        let environment: [UnsafeMutablePointer<CChar>?] =
            spec.environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer {
            argv.forEach { free($0) }
            environment.forEach { free($0) }
        }

        var master: Int32 = -1
        let pid = spec.cwd.withCString { cwd in
            executable.withCString { path in
                dirijor_pty_spawn(
                    path, argv, environment, cwd, spec.cols, spec.rows, &master)
            }
        }
        guard pid > 0, master >= 0 else {
            throw HolderError.launch("PTY spawn of \(executable) failed")
        }
        return (master, pid)
    }

    static func resize(master: Int32, cols: UInt16, rows: UInt16) {
        var size = winsize(ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0)
        _ = ioctl(master, TIOCSWINSZ, &size)
    }

    static func size(master: Int32) -> (cols: UInt16, rows: UInt16)? {
        var size = winsize()
        guard ioctl(master, TIOCGWINSZ, &size) == 0 else { return nil }
        return (size.ws_col, size.ws_row)
    }
}
