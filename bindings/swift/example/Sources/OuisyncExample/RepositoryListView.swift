import SwiftUI
import OuisyncLib

struct RepositoryListView: View {
    @EnvironmentObject private var viewModel: ExampleViewModel
    @State private var navigationPath: [Route] = []
    @State private var isCreating = false
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
            CreateRepositorySheet { name, token in
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
                RepositoryRow(name: name, onNavigate: {
                    navigationPath.append(.folder(repositoryName: name, path: ""))
                }, onShare: {
                    Task {
                        if let token = await viewModel.shareRepository(name: name) {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(token, forType: .string)
                        }
                    }
                }, onDelete: {
                    Task {
                        do {
                            try await viewModel.deleteRepository(name: name)
                        } catch {
                            errorMessage = error.localizedDescription
                        }
                    }
                })
            }
        }
    }
}

private struct RepositoryRow: View {
    let name: String
    let onNavigate: () -> Void
    let onShare: () -> Void
    let onDelete: () -> Void

    @State private var confirmDelete = false

    var body: some View {
        HStack {
            Image(systemName: "folder")
            Text(name)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .onTapGesture { onNavigate() }

            Button { onShare() } label: { Image(systemName: "square.and.arrow.up") }
                .buttonStyle(.borderless)
                .help("Copy share token to clipboard")

            Button { confirmDelete = true } label: { Image(systemName: "trash") }
                .buttonStyle(.borderless)
                .foregroundStyle(.red)
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
    let onSubmit: (String, String) -> Void
    let onCancel: () -> Void

    @State private var name = ""
    @State private var token = ""
    @State private var nameError = ""

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
        .frame(width: 320)
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
