import SwiftUI

@main
struct OuisyncExampleApp: App {
    @StateObject private var viewModel = ExampleViewModel()

    var body: some Scene {
        WindowGroup {
            RepositoryListView()
                .environmentObject(viewModel)
                .frame(minWidth: 400, minHeight: 300)
        }
        .commands {
            CommandGroup(replacing: .newItem) {}
        }
    }
}
