import DirijorCore
import DirijorProtocol
import Foundation

/// The daemon-owned browser pool: supervises a single Node + Playwright sidecar
/// that holds one long-lived browser per engine (chromium/webkit/firefox) and
/// hands each test run a cheap isolated context. This is the memory win — many
/// agents share 3 browser processes, not N Chromes.
///
/// Transport is newline-delimited JSON over the sidecar's stdio, multiplexed by
/// request id so concurrent `test.run`s don't block each other. The sidecar is
/// launched lazily on first use and **idle-recycled** (killed, RAM fully
/// reclaimed) after a quiet period; the next run relaunches it.
public actor BrowserPool {
    static var isAvailable: Bool {
        LoginEnvironment.resolve("node") != nil && locateSidecar() != nil
    }
    private let nodePath: String?
    private let sidecarJS: String?
    private let artifactDir: URL
    /// No runs for this long → kill the sidecar and reclaim all browser RAM.
    private let idleTimeout: Duration = .seconds(180)

    private var process: Process?
    private var stdin: FileHandle?
    private var readerTask: Task<Void, Never>?
    private var idleTask: Task<Void, Never>?
    private var pending: [Int: CheckedContinuation<JSONValue, Error>] = [:]
    private var nextID = 0
    private var lastActivity = ContinuousClock.now
    /// Sessions with an interactive browser currently open. A scripted
    /// `test.run` is self-contained and safe to recycle between runs, but an
    /// open page is state an agent is mid-way through using — reclaiming its
    /// RAM would drop the page silently and the next action would fail for a
    /// reason the agent cannot see.
    private var openBrowserSessions: Set<SessionID> = []

    public init(config: DaemonConfig) {
        self.nodePath = LoginEnvironment.resolve("node")
        self.sidecarJS = Self.locateSidecar()
        self.artifactDir = config.logsDir.appendingPathComponent("test-artifacts")
    }

    /// Nothing to do — the sidecar launches lazily on the first `run`.
    public func start() {}

    public func stop() { teardown(reason: "daemon shutdown") }

    // MARK: Public API

    /// Run a cross-browser flow. Returns the sidecar's structured result verbatim
    /// (`{ pass, results: { engine: {...} } }`) as a JSONValue.
    public func run(_ params: TestRunParams) async throws -> JSONValue {
        var obj: [String: JSONValue] = [
            "url": .string(params.url),
            "steps": .array(params.steps),
            "artifactDir": .string(artifactDir.path),
        ]
        if let engines = params.engines { obj["engines"] = .array(engines.map(JSONValue.string)) }
        if let observe = params.observe { obj["observe"] = .string(observe) }
        if let baseline = params.baseline { obj["baseline"] = .string(baseline) }
        if let profile = params.profile { obj["profile"] = .string(profile) }
        if let auth = params.auth { obj["auth"] = auth }
        return try await request(method: "run", params: .object(obj))
    }

    /// One step of an interactive, per-session browser. Mirrors `run` but keeps
    /// its page alive between calls — the sidecar holds the context keyed by
    /// session id.
    public func browse(_ params: BrowserParams) async throws -> JSONValue {
        var obj: [String: JSONValue] = [
            "sessionId": .string(params.sessionID.rawValue),
            "action": .string(params.action),
            "artifactDir": .string(artifactDir.path),
        ]
        func put(_ key: String, _ value: String?) {
            if let value { obj[key] = .string(value) }
        }
        func putNum(_ key: String, _ value: Double?) {
            if let value { obj[key] = .number(value) }
        }
        func putBool(_ key: String, _ value: Bool?) {
            if let value { obj[key] = .bool(value) }
        }
        put("url", params.url)
        put("ref", params.ref)
        put("selector", params.selector)
        put("text", params.text)
        put("key", params.key)
        put("value", params.value)
        put("what", params.what)
        put("state", params.state)
        put("direction", params.direction)
        put("button", params.button)
        put("engine", params.engine)
        put("profile", params.profile)
        putNum("ms", params.ms)
        putNum("amount", params.amount)
        putBool("double", params.double)
        putBool("full", params.full)
        putBool("annotate", params.annotate)

        let result = try await request(method: "browser", params: .object(obj))
        // Track liveness locally so the idle sweep never kills a sidecar that is
        // holding somebody's open page (see `recycleIfIdle`).
        switch params.action {
        case "open": openBrowserSessions.insert(params.sessionID)
        case "close": openBrowserSessions.remove(params.sessionID)
        default: break
        }
        return result
    }

    // MARK: Request/response over stdio

    private func request(method: String, params: JSONValue) async throws -> JSONValue {
        try ensureRunning()
        guard let stdin else {
            throw ControlError(code: "browser_pool", message: "sidecar not running")
        }
        lastActivity = .now
        nextID += 1
        let id = nextID
        let envelope: JSONValue = .object([
            "id": .number(Double(id)), "method": .string(method), "params": params,
        ])
        let line = try JSONEncoder.dirijor.encode(envelope) + Data([0x0a])
        return try await withCheckedThrowingContinuation { cont in
            pending[id] = cont
            do {
                try stdin.write(contentsOf: line)
            } catch {
                pending.removeValue(forKey: id)
                cont.resume(throwing: error)
            }
        }
    }

    private func handleLine(_ line: String) {
        lastActivity = .now
        guard let data = line.data(using: .utf8),
            let msg = try? JSONDecoder.dirijor.decode(JSONValue.self, from: data),
            case .number(let d)? = msg["id"]
        else { return }
        guard let cont = pending.removeValue(forKey: Int(d)) else { return }
        if case .string(let err)? = msg["error"] {
            cont.resume(throwing: ControlError(code: "browser_pool", message: err))
        } else {
            cont.resume(returning: msg["result"] ?? .null)
        }
    }

    // MARK: Lifecycle

    private func ensureRunning() throws {
        if process?.isRunning == true { return }
        guard let nodePath else {
            throw ControlError(
                code: "browser_pool",
                message: "node not found on PATH — install Node.js to use test_run")
        }
        guard let sidecarJS else {
            throw ControlError(code: "browser_pool", message: "test sidecar not found")
        }
        try? FileManager.default.createDirectory(at: artifactDir, withIntermediateDirectories: true)

        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: nodePath)
        proc.arguments = [sidecarJS]
        var env = ProcessInfo.processInfo.environment
        env["PATH"] = LoginEnvironment.path   // so node/playwright resolve like a login shell
        proc.environment = env

        let inPipe = Pipe(), outPipe = Pipe(), errPipe = Pipe()
        proc.standardInput = inPipe
        proc.standardOutput = outPipe
        proc.standardError = errPipe
        proc.terminationHandler = { [weak self] _ in
            Task { await self?.handleTermination() }
        }

        do {
            try proc.run()
        } catch {
            throw ControlError(code: "browser_pool", message: "failed to launch sidecar: \(error)")
        }

        process = proc
        stdin = inPipe.fileHandleForWriting

        let out = outPipe.fileHandleForReading
        readerTask = Task { [weak self] in
            do {
                for try await line in out.bytes.lines {
                    if Task.isCancelled { break }
                    await self?.handleLine(line)
                }
            } catch {}
        }
        let err = errPipe.fileHandleForReading
        Task {
            for try await line in err.bytes.lines {
                DaemonLog.shared.log("[test-sidecar] \(line)")
            }
        }
        startIdleTimer()
        DaemonLog.shared.log("browser pool launched sidecar (node: \(nodePath))")
    }

    private func handleTermination() {
        process = nil
        stdin = nil
        readerTask?.cancel()
        readerTask = nil
        failPending(reason: "sidecar exited")
    }

    private func startIdleTimer() {
        idleTask?.cancel()
        idleTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(30))
                guard let self else { return }
                await self.recycleIfIdle()
            }
        }
    }

    private func recycleIfIdle() {
        guard process?.isRunning == true, pending.isEmpty, openBrowserSessions.isEmpty,
            ContinuousClock.now - lastActivity > idleTimeout
        else { return }
        DaemonLog.shared.log("browser pool idle — recycling sidecar, reclaiming browser RAM")
        teardown(reason: "idle recycle")
    }

    private func teardown(reason: String) {
        idleTask?.cancel()
        idleTask = nil
        readerTask?.cancel()
        readerTask = nil
        process?.terminate()
        process = nil
        stdin = nil
        failPending(reason: reason)
    }

    private func failPending(reason: String) {
        // The sidecar going away takes every open page with it, so no session
        // can still be holding one — otherwise a stale entry here would block
        // idle recycling forever.
        openBrowserSessions.removeAll()
        let waiting = pending
        pending.removeAll()
        for (_, cont) in waiting {
            cont.resume(throwing: ControlError(code: "browser_pool", message: reason))
        }
    }

    // MARK: Sidecar discovery

    /// Finds `sidecar/server.js`: an explicit override, the app bundle Resources,
    /// or by walking up from the daemon executable (dev builds run from `.build`).
    static func locateSidecar() -> String? {
        let fm = FileManager.default
        if let override = ProcessInfo.processInfo.environment["DIRIJOR_SIDECAR"],
            fm.isReadableFile(atPath: override) { return override }

        var candidates: [String] = []
        if let res = Bundle.main.resourceURL {
            candidates.append(res.appendingPathComponent("sidecar/server.js").path)
            candidates.append(res.appendingPathComponent("server.js").path)
        }
        // `swift test` is hosted by xctest, whose command-line executable can
        // live under Xcode instead of this package. CI and local development
        // both launch from the repository, so search upward from cwd as well.
        var workingDir = URL(fileURLWithPath: fm.currentDirectoryPath, isDirectory: true)
        for _ in 0..<7 {
            candidates.append(workingDir.appendingPathComponent("sidecar/server.js").path)
            workingDir = workingDir.deletingLastPathComponent()
        }
        var dir = URL(fileURLWithPath: CommandLine.arguments.first ?? "/").deletingLastPathComponent()
        for _ in 0..<7 {
            candidates.append(dir.appendingPathComponent("sidecar/server.js").path)
            dir = dir.deletingLastPathComponent()
        }
        return candidates.first { fm.isReadableFile(atPath: $0) }
    }
}
