import SwiftUI

@main
struct TaskmasterCompanionApp: App {
    @StateObject private var model = CompanionModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
        }
    }
}
