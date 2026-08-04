import Darwin
import DirijorHolderKit
import Foundation

let arguments = CommandLine.arguments
do {
    // The daemon launches us with POSIX_SPAWN_SETSID. Direct/manual launches
    // detach here as well. A deliberate single process keeps launch health
    // observable; parent death never terminates a POSIX child on macOS.
    if getsid(0) != getpid() { _ = setsid() }
    signal(SIGHUP, SIG_IGN)

    if let managerIndex = arguments.firstIndex(of: "--manager"),
        managerIndex + 1 < arguments.count
    {
        let directory = URL(fileURLWithPath: arguments[managerIndex + 1], isDirectory: true)
        try HolderManagerServer(directory: directory).run()
    } else if let specIndex = arguments.firstIndex(of: "--spec"),
        specIndex + 1 < arguments.count
    {
        // Legacy/direct mode remains useful for compatibility tests and manual
        // recovery. Normal daemon launches go through the shared manager.
        let specURL = URL(fileURLWithPath: arguments[specIndex + 1])
        let data = try Data(contentsOf: specURL)
        let spec = try JSONDecoder().decode(HolderLaunchSpec.self, from: data)
        try? FileManager.default.removeItem(at: specURL)
        try HolderServer(spec: spec).run()
    } else {
        FileHandle.standardError.write(
            Data("usage: dirijord-holder --manager <directory> | --spec <path>\n".utf8))
        exit(64)
    }
} catch {
    FileHandle.standardError.write(Data("dirijord-holder: \(error)\n".utf8))
    exit(1)
}
