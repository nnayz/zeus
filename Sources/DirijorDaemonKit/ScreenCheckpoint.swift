import DirijorProtocol
import Foundation

/// A durable terminal checkpoint paired with the exact raw-log offset it
/// represents. The grid is already RLE encoded by DirijorProtocol, keeping a
/// mostly blank terminal to a few kilobytes instead of serializing every cell.
///
/// Checkpoints are an acceleration cache, never authoritative state: malformed,
/// stale, or future-version files are ignored and the bounded raw-log replay is
/// used instead.
struct ScreenCheckpoint: Codable {
    static let currentVersion = 1

    let version: Int
    let logOffset: UInt64
    let gridPayload: Data
    let markerBuffer: Data
    let altScreen: Bool
    let bracketedPaste: Bool
    let mouseReporting: Bool

    init(
        logOffset: UInt64,
        grid: GridUpdate,
        markerBuffer: Data = Data(),
        altScreen: Bool,
        bracketedPaste: Bool,
        mouseReporting: Bool
    ) {
        version = Self.currentVersion
        self.logOffset = logOffset
        gridPayload = grid.encoded()
        self.markerBuffer = markerBuffer
        self.altScreen = altScreen
        self.bracketedPaste = bracketedPaste
        self.mouseReporting = mouseReporting
    }

    var grid: GridUpdate? {
        guard version == Self.currentVersion else { return nil }
        return GridUpdate(decoding: gridPayload)
    }

    static func load(from url: URL) -> ScreenCheckpoint? {
        guard let data = try? Data(contentsOf: url),
            let checkpoint = try? PropertyListDecoder().decode(Self.self, from: data),
            checkpoint.version == currentVersion,
            checkpoint.grid != nil
        else { return nil }
        return checkpoint
    }

    func writeAtomically(to url: URL) throws {
        let data = try PropertyListEncoder().encode(self)
        try data.write(to: url, options: .atomic)
    }
}
