import SwiftUI
import OuisyncLib

struct FolderView: View {
    @EnvironmentObject private var viewModel: ExampleViewModel
    let repositoryName: String
    let path: String

    @State private var entries: [DirectoryEntry] = []
    @State private var error: String?
    @State private var isLoading = true
    @State private var isCreating = false
    @State private var errorMessage: String?

    private var repo: Repository? { viewModel.repositories[repositoryName] }

    var body: some View {
        Group {
            if isLoading {
                ProgressView()
            } else if let error {
                ContentUnavailableView(
                    "Error",
                    systemImage: "exclamationmark.triangle",
                    description: Text(error)
                )
            } else if entries.isEmpty {
                ContentUnavailableView(
                    "Empty Folder",
                    systemImage: "folder",
                    description: Text("Tap + to create a file or directory.")
                )
            } else {
                entryList
            }
        }
        .navigationTitle(path.isEmpty ? repositoryName : "\(repositoryName)\(path)")
        .task { await loadEntries() }
        .task(id: repositoryName) { await watchRepository() }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button { isCreating = true } label: { Image(systemName: "plus") }
            }
            ToolbarItem {
                Button { Task { await loadEntries() } } label: { Image(systemName: "arrow.clockwise") }
                    .help("Refresh")
            }
        }
        .sheet(isPresented: $isCreating) {
            CreateEntrySheet { name, kind in
                isCreating = false
                Task { await createEntry(name: name, kind: kind) }
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

    // MARK: - Entry list

    private var entryList: some View {
        List(entries, id: \.name) { entry in
            NavigationLink(value: destinationRoute(for: entry)) {
                Label(entry.name, systemImage: entry.entryType == .directory ? "folder" : "doc")
            }
        }
    }

    private func destinationRoute(for entry: DirectoryEntry) -> Route {
        let entryPath = "\(path)/\(entry.name)"
        switch entry.entryType {
        case .directory: return .folder(repositoryName: repositoryName, path: entryPath)
        case .file:      return .file(repositoryName: repositoryName, path: entryPath)
        }
    }

    // MARK: - Actions

    private func watchRepository() async {
        guard let repo else { return }
        guard let stream = try? await repo.subscribe() else { return }
        for await _ in stream {
            await loadEntries()
        }
    }

    private func loadEntries() async {
        guard let repo else {
            error = "Repository '\(repositoryName)' not found"
            isLoading = false
            return
        }
        isLoading = true
        error = nil
        do {
            entries = try await repo.readDirectory(path)
        } catch {
            self.error = error.localizedDescription
        }
        isLoading = false
    }

    private func createEntry(name: String, kind: EntryKind) async {
        guard let repo else { return }
        let entryPath = path.isEmpty ? "/\(name)" : "\(path)/\(name)"
        do {
            switch kind {
            case .file:
                let file = try await repo.createFile(entryPath)
                try await file.close()
            case .directory:
                try await repo.createDirectory(entryPath)
            }
            await loadEntries()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

// MARK: - Create entry sheet

enum EntryKind { case file, directory }

private struct CreateEntrySheet: View {
    let onSubmit: (String, EntryKind) -> Void
    let onCancel: () -> Void

    @State private var name = ""
    @State private var kind: EntryKind = .file
    @State private var nameError = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("New entry").font(.headline)

            Picker("Kind", selection: $kind) {
                Text("File").tag(EntryKind.file)
                Text("Directory").tag(EntryKind.directory)
            }
            .pickerStyle(.segmented)

            VStack(alignment: .leading, spacing: 4) {
                TextField("Name", text: $name)
                if !nameError.isEmpty {
                    Text(nameError).font(.caption).foregroundStyle(.red)
                }
            }

            HStack {
                Spacer()
                Button("Cancel") { onCancel() }
                Button("Create") {
                    guard validate() else { return }
                    onSubmit(name, kind)
                }
                .buttonStyle(.borderedProminent)
            }
        }
#if os(macOS)
        .frame(width: 280)
#endif
    }

    private func validate() -> Bool {
        if name.isEmpty { nameError = "Name is required"; return false }
        if name.contains("/") { nameError = "Name must not contain '/'"; return false }
        nameError = ""
        return true
    }
}
