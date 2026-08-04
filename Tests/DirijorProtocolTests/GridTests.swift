import Foundation
import Testing
@testable import DirijorProtocol

@Suite struct GridTests {
    @Test func roundTripsFullSnapshot() {
        let row0 = (y: 0, cells: [
            GridCell(scalar: 72, fg: .ansi(1), bg: .defaultInverted, style: [.bold]),
            GridCell(scalar: 105, fg: .rgb(10, 20, 30), bg: .defaultInverted, style: []),
        ] + Array(repeating: GridCell.blank, count: 8))
        let row1 = (y: 1, cells: Array(repeating: GridCell.blank, count: 10))
        let update = GridUpdate(
            cols: 10, rows: 2, cursorCol: 2, cursorRow: 0, cursorVisible: true,
            isFullSnapshot: true, changedRows: [row0, row1])

        let frame = Frame.grid(update)
        let decoded = frame.gridPayload
        #expect(decoded != nil)
        #expect(decoded?.cols == 10)
        #expect(decoded?.rows == 2)
        #expect(decoded?.cursorCol == 2)
        #expect(decoded?.isFullSnapshot == true)
        #expect(decoded?.changedRows.count == 2)
        #expect(decoded?.changedRows[0].cells.count == 10)
        #expect(decoded?.changedRows[0].cells[0] == row0.cells[0])
        #expect(decoded?.changedRows[0].cells[1] == row0.cells[1])
        #expect(decoded?.changedRows[0].cells[9] == GridCell.blank)
    }

    @Test func rleCompressesBlankRow() {
        let blankRow = (y: 5, cells: Array(repeating: GridCell.blank, count: 200))
        let update = GridUpdate(
            cols: 200, rows: 1, cursorCol: 0, cursorRow: 0, cursorVisible: false,
            isFullSnapshot: false, changedRows: [blankRow])
        #expect(update.encoded().count < 60)
        #expect(GridUpdate(decoding: update.encoded())?.changedRows[0].cells.count == 200)
    }
}
