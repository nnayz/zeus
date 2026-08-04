import DirijorCore
import DirijorProtocol
import Foundation
import XCTest

@testable import DirijorDaemonKit

/// End-to-end check of the browser pool → Playwright sidecar path. Launches a
/// real browser, so it's gated behind an env flag (needs `node` + installed
/// Playwright engines) and skipped in normal CI.
final class BrowserPoolTests: XCTestCase {
    func testRunAcrossEnginesReportsPass() async throws {
        guard ProcessInfo.processInfo.environment["DIRIJOR_RUN_BROWSER_TESTS"] == "1" else {
            throw XCTSkip("set DIRIJOR_RUN_BROWSER_TESTS=1 (needs node + playwright browsers)")
        }

        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }

        let page = tmp.appendingPathComponent("page.html")
        try """
            <!doctype html><meta charset=utf8>
            <input id=name>
            <button id=go onclick="document.getElementById('out').textContent='Hi '+document.getElementById('name').value">Go</button>
            <div id=out></div>
            """.write(to: page, atomically: true, encoding: .utf8)

        let config = DaemonConfig(
            socketPath: tmp.appendingPathComponent("s.sock").path,
            cliPath: "",
            injectDir: tmp,
            logsDir: tmp,
            stateFile: tmp.appendingPathComponent("state.json"))
        let pool = BrowserPool(config: config)

        let steps: [JSONValue] = [
            .object(["fill": .array([.string("#name"), .string("Dirijor")])]),
            .object(["click": .string("#go")]),
            .object(["assert": .object(["selector": .string("#out"), "text": .string("Hi Dirijor")])]),
        ]
        let result = try await pool.run(
            TestRunParams(url: "file://\(page.path)", engines: ["chromium", "webkit", "firefox"], steps: steps))
        await pool.stop()

        XCTAssertEqual(result["pass"], .bool(true), "expected all engines to pass: \(result)")
    }

    /// The auth hand-off: localStorage seeded via `auth` must be visible to the
    /// page's own scripts on first load, in every engine.
    func testAuthSeedsLocalStorageBeforeFirstLoad() async throws {
        guard ProcessInfo.processInfo.environment["DIRIJOR_RUN_BROWSER_TESTS"] == "1" else {
            throw XCTSkip("set DIRIJOR_RUN_BROWSER_TESTS=1 (needs node + playwright browsers)")
        }

        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }

        let page = tmp.appendingPathComponent("authed.html")
        try """
            <!doctype html><meta charset=utf8>
            <div id=tok></div>
            <script>document.getElementById('tok').textContent = localStorage.getItem('session') || 'logged-out'</script>
            """.write(to: page, atomically: true, encoding: .utf8)

        let config = DaemonConfig(
            socketPath: tmp.appendingPathComponent("s.sock").path,
            cliPath: "",
            injectDir: tmp,
            logsDir: tmp,
            stateFile: tmp.appendingPathComponent("state.json"))
        let pool = BrowserPool(config: config)

        let steps: [JSONValue] = [
            .object(["assert": .object(["selector": .string("#tok"), "text": .string("tok-123")])])
        ]
        let auth: JSONValue = .object([
            "localStorage": .object(["session": .string("tok-123")])
        ])
        let result = try await pool.run(
            TestRunParams(
                url: "file://\(page.path)", engines: ["chromium", "webkit", "firefox"],
                steps: steps, auth: auth))
        await pool.stop()

        XCTAssertEqual(result["pass"], .bool(true), "expected auth-seeded pass in all engines: \(result)")
    }

    // MARK: - Interactive browser

    private func makePool(_ tmp: URL) -> BrowserPool {
        BrowserPool(
            config: DaemonConfig(
                socketPath: tmp.appendingPathComponent("s.sock").path,
                cliPath: "",
                injectDir: tmp,
                logsDir: tmp,
                stateFile: tmp.appendingPathComponent("state.json")))
    }

    private func tempDir() throws -> URL {
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        return tmp
    }

    /// Runs `body` and always shuts the pool down afterwards. Swift has no async
    /// `defer`, and a fire-and-forget `defer { Task { await pool.stop() } }`
    /// does not work here: the sidecar outlives the test, keeps the inherited
    /// stdout pipe open, and `swift test` then hangs forever waiting on it.
    private func withPool(_ tmp: URL, _ body: (BrowserPool) async throws -> Void) async throws {
        let pool = makePool(tmp)
        do {
            try await body(pool)
        } catch {
            await pool.stop()
            throw error
        }
        await pool.stop()
    }

    /// The whole interactive loop: open returns refs, acting by ref changes the
    /// page, and the snapshot that comes back reflects the change.
    func testInteractiveBrowserDrivesPageByRef() async throws {
        guard ProcessInfo.processInfo.environment["DIRIJOR_RUN_BROWSER_TESTS"] == "1" else {
            throw XCTSkip("set DIRIJOR_RUN_BROWSER_TESTS=1 (needs node + playwright browsers)")
        }
        let tmp = try tempDir()
        defer { try? FileManager.default.removeItem(at: tmp) }

        let page = tmp.appendingPathComponent("form.html")
        try """
            <!doctype html><meta charset=utf8><title>Form</title>
            <label for=who>Your name</label><input id=who placeholder="e.g. Ada">
            <button id=go onclick="out.textContent='Hi '+who.value;console.error('kaboom')">Greet</button>
            <div id=out></div>
            """.write(to: page, atomically: true, encoding: .utf8)

        let sid = SessionID(rawValue: "s_browser_test")
        try await withPool(tmp) { pool in
            let opened = try await pool.browse(
                BrowserParams(sessionID: sid, action: "open", url: "file://\(page.path)"))
            let snapshot = opened["snapshot"]?.stringValue ?? ""
            // The <label> must win over the placeholder — the name is how a model
            // picks the field, and "e.g. Ada" names the example, not the input.
            XCTAssertTrue(snapshot.contains("\"Your name\""), "expected label-derived name: \(snapshot)")
            XCTAssertFalse(snapshot.contains("e.g. Ada"), "placeholder should not win over label: \(snapshot)")
            XCTAssertTrue(snapshot.contains("@e1"), "expected stamped refs: \(snapshot)")

            _ = try await pool.browse(
                BrowserParams(sessionID: sid, action: "fill", ref: "e1", text: "Ada"))
            let clicked = try await pool.browse(BrowserParams(sessionID: sid, action: "click", ref: "e2"))
            // Acting returns a fresh snapshot, so the caller can't act on stale refs.
            XCTAssertNotNil(clicked["snapshot"], "every action should return a new snapshot")

            let text = try await pool.browse(
                BrowserParams(sessionID: sid, action: "get", selector: "#out", what: "text"))
            XCTAssertEqual(text["text"]?.stringValue, "Hi Ada")

            let console = try await pool.browse(BrowserParams(sessionID: sid, action: "console"))
            let consoleText = String(
                data: (try? JSONEncoder.dirijor.encode(console)) ?? Data(), encoding: .utf8) ?? ""
            XCTAssertTrue(
                consoleText.contains("kaboom"),
                "page console errors should be captured: \(consoleText)")

            _ = try await pool.browse(BrowserParams(sessionID: sid, action: "close"))
        }
    }

    /// A ref that no longer resolves must say *why* — an agent told only "not
    /// found" concludes the element never existed and starts guessing selectors.
    func testStaleRefExplainsItself() async throws {
        guard ProcessInfo.processInfo.environment["DIRIJOR_RUN_BROWSER_TESTS"] == "1" else {
            throw XCTSkip("set DIRIJOR_RUN_BROWSER_TESTS=1 (needs node + playwright browsers)")
        }
        let tmp = try tempDir()
        defer { try? FileManager.default.removeItem(at: tmp) }

        let page = tmp.appendingPathComponent("p.html")
        try "<!doctype html><button>only</button>".write(to: page, atomically: true, encoding: .utf8)

        let sid = SessionID(rawValue: "s_stale_test")
        try await withPool(tmp) { pool in
            _ = try await pool.browse(
                BrowserParams(sessionID: sid, action: "open", url: "file://\(page.path)"))
            do {
                _ = try await pool.browse(BrowserParams(sessionID: sid, action: "click", ref: "e99"))
                XCTFail("clicking a nonexistent ref should throw")
            } catch {
                XCTAssertTrue(
                    "\(error)".contains("stale"), "error should explain staleness: \(error)")
        }
        _ = try await pool.browse(BrowserParams(sessionID: sid, action: "close"))
        }
    }

    /// Two sessions on the same URL must get independent pages: typing in one
    /// is invisible to the other, and closing one leaves the other alive. The
    /// stronger promise underneath — separate cookie jars and logins — follows
    /// from each session owning a distinct Playwright context, which is what
    /// this asserts at the level the pool actually controls.
    func testSessionsGetIndependentPages() async throws {
        guard ProcessInfo.processInfo.environment["DIRIJOR_RUN_BROWSER_TESTS"] == "1" else {
            throw XCTSkip("set DIRIJOR_RUN_BROWSER_TESTS=1 (needs node + playwright browsers)")
        }
        let tmp = try tempDir()
        defer { try? FileManager.default.removeItem(at: tmp) }

        let page = tmp.appendingPathComponent("iso.html")
        try "<!doctype html><meta charset=utf8><input id=box placeholder=type-here>"
            .write(to: page, atomically: true, encoding: .utf8)

        let a = SessionID(rawValue: "s_iso_a")
        let b = SessionID(rawValue: "s_iso_b")
        try await withPool(tmp) { pool in
            let url = "file://\(page.path)"
            _ = try await pool.browse(BrowserParams(sessionID: a, action: "open", url: url))
            _ = try await pool.browse(BrowserParams(sessionID: b, action: "open", url: url))

            _ = try await pool.browse(
                BrowserParams(sessionID: a, action: "fill", ref: "e1", text: "only-in-a"))

            let aValue = try await pool.browse(
                BrowserParams(sessionID: a, action: "get", ref: "e1", what: "value"))
            XCTAssertEqual(aValue["value"]?.stringValue, "only-in-a")

            let bValue = try await pool.browse(
                BrowserParams(sessionID: b, action: "get", ref: "e1", what: "value"))
            XCTAssertEqual(bValue["value"]?.stringValue, "", "session B must not see session A's typing")

            // Closing one session must not take the other's page down with it.
            _ = try await pool.browse(BrowserParams(sessionID: a, action: "close"))
            let bStillAlive = try await pool.browse(BrowserParams(sessionID: b, action: "snapshot"))
            XCTAssertNotNil(bStillAlive["snapshot"], "closing A should leave B usable")

            _ = try await pool.browse(BrowserParams(sessionID: b, action: "close"))
        }
    }
}
