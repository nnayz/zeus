import DirijorCore
import Foundation
import Testing

@testable import DirijorProtocol

@Suite struct ScrollbackWireTests {
    @Test func readScrollbackResultRoundTrips() throws {
        let result = ReadScrollbackResult(
            lines: ["first", "second", "third"],
            firstRow: 100, visibleStartRow: 101, cols: 80, rows: 2,
            contentSeq: 42, isAltScreen: true)
        let data = try JSONEncoder.dirijor.encode(result)
        let back = try JSONDecoder.dirijor.decode(ReadScrollbackResult.self, from: data)
        #expect(back.lines == result.lines)
        #expect(back.firstRow == 100)
        #expect(back.visibleStartRow == 101)
        #expect(back.cols == 80)
        #expect(back.rows == 2)
        #expect(back.contentSeq == 42)
        #expect(back.isAltScreen)
    }

    @Test func readScrollbackCellsParamsRoundTrips() throws {
        let params = ReadScrollbackCellsParams(
            sessionID: SessionID(rawValue: "s_x"), firstRow: 7, maxRows: 50)
        let data = try JSONEncoder.dirijor.encode(params)
        let back = try JSONDecoder.dirijor.decode(ReadScrollbackCellsParams.self, from: data)
        #expect(back.sessionID.rawValue == "s_x")
        #expect(back.firstRow == 7)
        #expect(back.maxRows == 50)
    }

    @Test func readScrollbackCellsResultCarriesCellsThroughSharedCodec() throws {
        // Two rows encoded with the SAME RLE row codec grid frames use.
        let rowA: [GridCell] = [
            GridCell(scalar: 65, fg: .ansi(2), bg: .defaultInverted, style: [.bold]),
            GridCell(scalar: 66, fg: .rgb(1, 2, 3), bg: .defaultInverted, style: []),
        ] + Array(repeating: GridCell.blank, count: 3)
        let rowB = Array(repeating: GridCell.blank, count: 5)
        let payload = GridRowCodec.encodeRows([rowA, rowB])
        let result = ReadScrollbackCellsResult(
            payload: payload, firstRow: 10, rowCount: 2, totalRows: 12,
            liveStartRow: 8, cols: 5, contentSeq: 3)

        // Data survives the JSON hop (base64) and decodes back to identical cells.
        let json = try JSONEncoder.dirijor.encode(result)
        let back = try JSONDecoder.dirijor.decode(ReadScrollbackCellsResult.self, from: json)
        let rows = try #require(back.decodedRows())
        #expect(rows.count == 2)
        #expect(rows[0] == rowA)
        #expect(rows[1] == rowB)
        #expect(back.totalRows == 12)
        #expect(back.liveStartRow == 8)
    }
}
