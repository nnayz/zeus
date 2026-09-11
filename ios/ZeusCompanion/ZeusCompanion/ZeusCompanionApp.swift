import SwiftUI

@main
struct ZeusCompanionApp: App {
    @StateObject private var model = CompanionModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
        }
    }
}
