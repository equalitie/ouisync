import SwiftUI

@main
struct OuisyncExampleApp: App {
    @StateObject private var viewModel = ExampleViewModel()

    var body: some Scene {
        WindowGroup {
            RepositoryListView()
                .environmentObject(viewModel)
                .onOpenURL { url in
                    viewModel.pendingShare = PendingShare(url: url)
                }
#if os(macOS)
                .frame(minWidth: 400, minHeight: 300)
#endif
        }
#if os(macOS)
        .commands {
            CommandGroup(replacing: .newItem) {}
        }
#endif
    }
}
