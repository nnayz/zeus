import DirijorProtocol
import Foundation
import Testing

@testable import DirijorDaemonKit

@Test func unattendedSessionsUseTheBackgroundStatusCadence() {
    #expect(StatusEngine.shouldInspectSession(attached: true, appActive: true, tick: 1))
    #expect(!StatusEngine.shouldInspectSession(attached: false, appActive: true, tick: 1))
    #expect(StatusEngine.shouldInspectSession(attached: false, appActive: true, tick: 3))
    #expect(StatusEngine.shouldInspectSession(attached: false, appActive: false, tick: 1))
    #expect(StatusEngine.activeTickInterval == .milliseconds(200))
    #expect(StatusEngine.backgroundTickInterval == .seconds(1))
}

@Test func portDiscoveryOnlyRunsForVisibleSessionsAtItsSlowCadence() {
    #expect(!ResourceGovernor.shouldScanPorts(enabled: true, attached: false, tick: 4))
    #expect(!ResourceGovernor.shouldScanPorts(enabled: true, attached: true, tick: 1))
    #expect(ResourceGovernor.shouldScanPorts(enabled: true, attached: true, tick: 4))
}

@Test func resourceGovernorAppliesRuntimeSettings() async {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("dirijor-governor-\(UUID().uuidString)")
    let config = DaemonConfig(
        socketPath: root.appendingPathComponent("d.sock").path,
        cliPath: "/usr/bin/true", injectDir: root, logsDir: root,
        stateFile: root.appendingPathComponent("state.json"))
    let registry = SessionRegistry(config: config, events: EventBus())
    let governor = ResourceGovernor(registry: registry)

    await governor.configure(
        GovernorSettingsParams(idleThresholdSeconds: 300, hardMemoryBytes: 4 << 30))
    let applied = await governor.currentConfig()

    #expect(applied.idleThresholdSeconds == 300)
    #expect(applied.hardMemoryBytes == 4 << 30)
}
