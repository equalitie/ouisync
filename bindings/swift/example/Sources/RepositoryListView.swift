import SwiftUI
import OuisyncLib

struct RepositoryListView: View {
    @EnvironmentObject private var viewModel: ExampleViewModel
    @State private var navigationPath: [Route] = []
    @State private var isCreating = false
    @State private var initialName = ""
    @State private var initialToken = ""
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack(path: $navigationPath) {
            content
                .navigationTitle("Repositories")
                .toolbar {
                    ToolbarItem(placement: .primaryAction) {
                        Button { isCreating = true } label: { Image(systemName: "plus") }
                    }
                }
                .navigationDestination(for: Route.self) { route in
                    switch route {
                    case .folder(let repoName, let path):
                        FolderView(repositoryName: repoName, path: path)
                            .environmentObject(viewModel)
                    case .file(let repoName, let path):
                        FileView(repositoryName: repoName, path: path)
                            .environmentObject(viewModel)
                    }
                }
        }
        .sheet(isPresented: $isCreating) {
            CreateRepositorySheet(initialName: initialName, initialToken: initialToken) { name, token in
                isCreating = false
                Task {
                    do {
                        try await viewModel.createRepository(name: name, token: token)
                    } catch {
                        errorMessage = error.localizedDescription
                    }
                }
            } onCancel: {
                isCreating = false
            }
            .padding()
        }
        .onChange(of: viewModel.pendingShare) { share in
            guard let share else { return }
            initialName = share.suggestedName
            initialToken = share.token
            isCreating = true
            viewModel.pendingShare = nil
        }
        .alert("Error", isPresented: Binding(
            get: { errorMessage != nil },
            set: { if !$0 { errorMessage = nil } }
        )) {
            Button("OK") { errorMessage = nil }
        } message: {
            Text(errorMessage ?? "")
        }
    }

    @ViewBuilder
    private var content: some View {
        if let error = viewModel.sessionError {
            ContentUnavailableView("Session Error", systemImage: "exclamationmark.triangle", description: Text(error))
        } else if viewModel.repositories.isEmpty {
            ContentUnavailableView("No Repositories", systemImage: "folder", description: Text("Tap + to create a new repository."))
        } else {
            List(Array(viewModel.repositories.keys.sorted()), id: \.self) { name in
                RepositoryRow(
                    name: URL(fileURLWithPath: name).deletingPathExtension().lastPathComponent,
                    infoHash: viewModel.repositoryInfoHashes[name],
                    onNavigate: {
                        navigationPath.append(.folder(repositoryName: name, path: ""))
                    },
                    onShare: {
                        await viewModel.shareRepository(name: name)
                    },
                    onDelete: {
                        Task {
                            do {
                                try await viewModel.deleteRepository(name: name)
                            } catch {
                                errorMessage = error.localizedDescription
                            }
                        }
                    }
                )
            }
        }
    }
}

private struct RepositoryRow: View {
    let name: String
    let infoHash: String?
    let onNavigate: () -> Void
    let onShare: () async -> String?
    let onDelete: () -> Void

    @State private var shareToken: String?
    @State private var confirmDelete = false

    var body: some View {
        HStack {
            Image(systemName: "folder")
            VStack(alignment: .leading, spacing: 2) {
                Text(name)
                if let hash = infoHash {
                    Text("id: \(hash.prefix(16))…")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .monospaced()
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .onTapGesture { onNavigate() }

            Group {
                if let shareURL = shareToken.flatMap(ouisyncURL(from:)) {
                    ShareLink(item: shareURL) {
                        Image(systemName: "square.and.arrow.up")
                    }
                } else {
                    Image(systemName: "square.and.arrow.up")
                        .foregroundStyle(.tertiary)
                }
            }
            .buttonStyle(.borderless)
            .help("Share repository")

            Button { confirmDelete = true } label: { Image(systemName: "trash") }
                .buttonStyle(.borderless)
                .foregroundStyle(.red)
        }
        .task {
            shareToken = await onShare()
        }
        .confirmationDialog("Delete \"\(name)\"?", isPresented: $confirmDelete) {
            Button("Delete", role: .destructive) { onDelete() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This will permanently delete the repository and all its contents.")
        }
    }
}

private struct CreateRepositorySheet: View {
    var initialName: String = ""
    var initialToken: String = ""
    let onSubmit: (String, String) -> Void
    let onCancel: () -> Void

    @State private var name: String
    @State private var token: String
    @State private var nameError = ""

    init(initialName: String = "", initialToken: String = "", onSubmit: @escaping (String, String) -> Void, onCancel: @escaping () -> Void) {
        self.initialName = initialName
        self.initialToken = initialToken
        self.onSubmit = onSubmit
        self.onCancel = onCancel
        _name = State(initialValue: initialName)
        _token = State(initialValue: initialToken)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Create Repository").font(.headline)

            VStack(alignment: .leading, spacing: 4) {
                TextField("Name *", text: $name)
                if !nameError.isEmpty {
                    Text(nameError).font(.caption).foregroundStyle(.red)
                }
            }

            TextField("Share token (optional)", text: $token)

            HStack {
                Spacer()
                Button("Cancel") { onCancel() }
                Button("Create") {
                    guard validate() else { return }
                    onSubmit(name, token)
                }
                .buttonStyle(.borderedProminent)
            }
        }
#if os(macOS)
        .frame(width: 320)
#endif
    }

    private func validate() -> Bool {
        if name.isEmpty {
            nameError = "Name is required"
            return false
        }
        nameError = ""
        return true
    }
}

enum Route: Hashable {
    case folder(repositoryName: String, path: String)
    case file(repositoryName: String, path: String)
}

// Converts https://ouisync.net/... → ouisync://ouisync.net/... for sharing.
// AirDrop and other share targets receive the ouisync:// URL; the app converts
// it back to https:// when validating the token on the receiving side.
private func ouisyncURL(from token: String) -> URL? {
    guard var components = URLComponents(string: token) else { return nil }
    components.scheme = "ouisync"
    return components.url
}
