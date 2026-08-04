import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

@Test func screenCheckpointRoundTripsGridAndOffset() throws {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-checkpoint-tests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    let url = directory.appendingPathComponent("screen.plist")
    let grid = GridUpdate(
        cols: 3,
        rows: 1,
        cursorCol: 2,
        cursorRow: 0,
        cursorVisible: false,
        isFullSnapshot: true,
        changedRows: [(0, [
            GridCell(scalar: 65, fg: .ansi(2), bg: .defaultInverted, style: .bold),
            .blank,
            GridCell(scalar: 66, fg: .rgb(1, 2, 3), bg: .ansi(4), style: .italic),
        ])])
    let checkpoint = ScreenCheckpoint(
        logOffset: 42,
        grid: grid,
        markerBuffer: Data([1, 2]),
        altScreen: true,
        bracketedPaste: true,
        mouseReporting: false)

    try checkpoint.writeAtomically(to: url)
    let restored = try #require(ScreenCheckpoint.load(from: url))
    let restoredGrid = try #require(restored.grid)
    #expect(restored.logOffset == 42)
    #expect(restored.markerBuffer == Data([1, 2]))
    #expect(restored.altScreen)
    #expect(restored.bracketedPaste)
    #expect(restoredGrid.encoded() == grid.encoded())
}

@Test func headlessScreenRestoresPersistedVisibleGrid() throws {
    let original = HeadlessScreen(cols: 20, rows: 4)
    _ = original.feed(Data("\u{1B}[2J\u{1B}[1;1Hplain\u{1B}[2;1H\u{1B}[31;1mstyled\u{1B}[0m".utf8))
    let grid = original.fullSnapshot()

    let restored = HeadlessScreen(cols: 20, rows: 4)
    #expect(
        restored.restore(
            checkpoint: grid,
            altScreen: original.isAltScreen,
            bracketedPaste: original.bracketedPasteActive,
            mouseReporting: original.mouseReporting))
    #expect(
        restored.snapshot().lines.map { $0.trimmingCharacters(in: .whitespaces) }
            == original.snapshot().lines.map { $0.trimmingCharacters(in: .whitespaces) })
    #expect(restored.fullSnapshot().encoded() == grid.encoded())
}
