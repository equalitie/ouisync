import SwiftUI
import UniformTypeIdentifiers
import OuisyncLib

struct FolderView: View {
    @EnvironmentObject private var viewModel: ExampleViewModel
    let repositoryName: String
    let path: String

    @State private var entries: [DirectoryEntry] = []
    @State private var error: String?
    @State private var isLoading = true
    @State private var isCreatingFolder = false
    @State private var isImporting = false
    @State private var importProgress: (name: String, fraction: Double)?
    @State private var pendingDelete: DirectoryEntry?
    @State private var errorMessage: String?
#if os(macOS)
    @State private var isDropTargeted = false
#endif

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
                    description: Text("Tap + to import a file, or the folder button to create a directory.")
                )
            } else {
                entryList
            }
        }
        .navigationTitle(path.isEmpty ? repositoryName : "\(repositoryName)\(path)")
        .task { await loadEntries() }
        .task(id: repositoryName) { await watchRepository() }
        .overlay { if let importProgress { importOverlay(importProgress) } }
#if os(macOS)
        .dropDestination(for: URL.self) { urls, _ in
            let files = urls.filter { $0.isFileURL }
            guard !files.isEmpty, importProgress == nil else { return false }
            Task { await importFiles(files) }
            return true
        } isTargeted: { isDropTargeted = $0 }
        .overlay {
            if isDropTargeted {
                ZStack {
                    RoundedRectangle(cornerRadius: 16)
                        .strokeBorder(Color.accentColor, lineWidth: 3)
                        .background(Color.accentColor.opacity(0.08))
                    Image(systemName: "arrow.down.circle.dotted")
                        .font(.system(size: 72))
                        .foregroundStyle(Color.accentColor)
                }
                .allowsHitTesting(false)
            }
        }
#endif
        .toolbar {
            ToolbarItem {
                Button { isCreatingFolder = true } label: { Label("New folder", systemImage: "folder.badge.plus") }
                    .help("New folder")
            }
            ToolbarItem {
                Button { isImporting = true } label: { Label("Import a file", systemImage: "plus") }
                    .help("Import a file")
                    .disabled(importProgress != nil)
            }
            ToolbarItem {
                Button { Task { await loadEntries() } } label: { Label("Refresh", systemImage: "arrow.clockwise") }
                    .help("Refresh")
            }
        }
        .fileImporter(
            isPresented: $isImporting,
            allowedContentTypes: [.item],
            allowsMultipleSelection: true
        ) { result in
            switch result {
            case .success(let urls):
                Task { await importFiles(urls) }
            case .failure(let error):
                errorMessage = error.localizedDescription
            }
        }
        .sheet(isPresented: $isCreatingFolder) {
            NewFolderSheet { name in
                isCreatingFolder = false
                Task { await createFolder(name: name) }
            } onCancel: {
                isCreatingFolder = false
            }
        }
        .alert("Error", isPresented: Binding(
            get: { errorMessage != nil },
            set: { if !$0 { errorMessage = nil } }
        )) {
            Button("OK") { errorMessage = nil }
        } message: {
            Text(errorMessage ?? "")
        }
        .confirmationDialog(
            pendingDelete.map { "Delete \"\($0.name)\"?" } ?? "",
            isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } }),
            titleVisibility: .visible
        ) {
            if let entry = pendingDelete {
                Button("Delete", role: .destructive) {
                    pendingDelete = nil
                    Task { await deleteEntry(entry) }
                }
            }
            Button("Cancel", role: .cancel) { pendingDelete = nil }
        } message: {
            if pendingDelete?.entryType == .directory {
                Text("The folder and all its contents will be deleted.")
            }
        }
    }

    // MARK: - Entry list

    private var entryList: some View {
        List(entries, id: \.name) { entry in
            NavigationLink(value: destinationRoute(for: entry)) {
                Label(entry.name, systemImage: entry.entryType == .directory ? "folder" : "doc")
            }
            .swipeActions(edge: .trailing) {
                Button(role: .destructive) { pendingDelete = entry } label: {
                    Label("Delete", systemImage: "trash")
                }
            }
            .contextMenu {
                Button(role: .destructive) { pendingDelete = entry } label: {
                    Label("Delete", systemImage: "trash")
                }
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

    private func createFolder(name: String) async {
        guard let repo else { return }
        let entryPath = path.isEmpty ? "/\(name)" : "\(path)/\(name)"
        do {
            try await repo.createDirectory(entryPath)
            await loadEntries()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func deleteEntry(_ entry: DirectoryEntry) async {
        guard let repo else { return }
        let entryPath = path.isEmpty ? "/\(entry.name)" : "\(path)/\(entry.name)"
        do {
            switch entry.entryType {
            case .file:      try await repo.removeFile(entryPath)
            case .directory: try await repo.removeDirectory(entryPath, true)
            }
            await loadEntries()
        } catch {
            errorMessage = "Failed to delete \(entry.name): \(error.localizedDescription)"
        }
    }

    // MARK: - Import

    private func importOverlay(_ progress: (name: String, fraction: Double)) -> some View {
        ZStack {
            Color.black.opacity(0.2).ignoresSafeArea()
            VStack(spacing: 12) {
                ProgressView(value: progress.fraction) {
                    Text("Importing \(progress.name)…")
                }
                Text("\(Int(progress.fraction * 100))%").foregroundStyle(.secondary)
            }
            .padding(24)
            .frame(maxWidth: 320)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
        }
    }

    private func importFiles(_ urls: [URL]) async {
        for url in urls {
            await importFile(url)
        }
        importProgress = nil
        await loadEntries()
    }

    private func importFile(_ url: URL) async {
        guard let repo else { return }

        let name = url.lastPathComponent
        let entryPath = path.isEmpty ? "/\(name)" : "\(path)/\(name)"

        // Files chosen outside the app sandbox are security-scoped.
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }

        importProgress = (name: name, fraction: 0)

        do {
            let handle = try FileHandle(forReadingFrom: url)
            defer { try? handle.close() }

            let totalBytes = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).flatMap { UInt64($0) }

            let file = try await repo.createFile(entryPath)
            defer { Task { try? await file.close() } }

            let chunkSize = 65536
            var offset: UInt64 = 0

            while true {
                let chunk = try handle.read(upToCount: chunkSize) ?? Data()
                if chunk.isEmpty { break }
                try await file.write(offset, chunk)
                offset += UInt64(chunk.count)

                if let totalBytes, totalBytes > 0 {
                    importProgress = (name: name, fraction: Double(offset) / Double(totalBytes))
                }
            }

            try await file.flush()
        } catch {
            errorMessage = "Failed to import \(name): \(error.localizedDescription)"
        }
    }
}

// MARK: - New folder sheet

private struct NewFolderSheet: View {
    let onSubmit: (String) -> Void
    let onCancel: () -> Void

    @State private var name = ""
    @State private var nameError = ""

    var body: some View {
#if os(iOS)
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $name)
                    if !nameError.isEmpty {
                        Text(nameError).font(.caption).foregroundStyle(.red)
                    }
                } header: {
                    Text("Name")
                }
            }
            .navigationTitle("New Folder")
            .navigationBarTitleDisplayMode(.inline)
            .safeAreaInset(edge: .bottom) {
                HStack(spacing: 12) {
                    Button(role: .cancel) { onCancel() } label: {
                        Text("Cancel").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)

                    Button {
                        guard validate() else { return }
                        onSubmit(name)
                    } label: {
                        Text("Create").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                }
                .controlSize(.large)
                .padding()
                .background(.bar)
            }
        }
        .presentationDetents([.medium])
        .presentationDragIndicator(.visible)
#else
        VStack(alignment: .leading, spacing: 16) {
            Text("New folder").font(.headline)

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
                    onSubmit(name)
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding()
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
