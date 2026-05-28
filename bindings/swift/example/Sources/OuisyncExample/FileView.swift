import SwiftUI
import CryptoKit
import OuisyncLib

struct FileView: View {
    @EnvironmentObject private var viewModel: ExampleViewModel
    let repositoryName: String
    let path: String

    @State private var state: FileState = .loading
    @State private var isWriting = false
    @State private var errorMessage: String?

    private var repo: Repository? { viewModel.repositories[repositoryName] }

    var body: some View {
        Group {
            switch state {
            case .loading:
                ProgressView("Loading…")
            case .syncing(let progress):
                VStack(spacing: 12) {
                    ProgressView("Syncing…", value: progress)
                    Text("\(Int(progress * 100))%").foregroundStyle(.secondary)
                }
                .padding()
            case .reading(let progress):
                VStack(spacing: 12) {
                    ProgressView("Reading…", value: progress)
                    Text("\(Int(progress * 100))%").foregroundStyle(.secondary)
                }
                .padding()
            case .done(let info):
                fileInfoTable(info)
            case .error(let message):
                ContentUnavailableView(
                    "Error",
                    systemImage: "exclamationmark.triangle",
                    description: Text(message)
                )
            }
        }
        .navigationTitle((path as NSString).lastPathComponent)
        .task { await loadFile() }
        .toolbar {
            ToolbarItem {
                Button { isWriting = true } label: { Image(systemName: "pencil") }
                    .help("Write text content")
            }
            ToolbarItem {
                Button { Task { await loadFile() } } label: { Image(systemName: "arrow.clockwise") }
                    .help("Refresh")
            }
        }
        .sheet(isPresented: $isWriting) {
            WriteContentSheet { text in
                isWriting = false
                Task { await writeContent(text) }
            } onCancel: {
                isWriting = false
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

    // MARK: - Info table

    private func fileInfoTable(_ info: FileInfo) -> some View {
        Form {
            LabeledContent("Size", value: formatSize(info.length))
            if !info.fullySync {
                LabeledContent("Synced", value: "\(info.syncedBytes) / \(info.length) bytes")
            }
            if !info.sha256.isEmpty {
                LabeledContent("SHA-256", value: info.sha256)
            }
            if let text = info.text {
                Section("Content") {
                    Text(text)
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                }
            }
        }
        .formStyle(.grouped)
    }

    // MARK: - Load

    private func loadFile() async {
        guard let repo else {
            state = .error("Repository '\(repositoryName)' not found")
            return
        }

        state = .loading

        do {
            let file = try await repo.openFile(path)
            defer { Task { try? await file.close() } }

            let length = try await file.getLength()

            // Poll for sync progress until the file is fully downloaded.
            // (subscribe() is not yet wired in the Swift bindings.)
            while true {
                let synced = try await file.getProgress()
                let progress = length > 0 ? Double(synced) / Double(length) : 1.0
                state = .syncing(progress)
                if synced >= length { break }
                try await Task.sleep(for: .seconds(1))
            }

            if length == 0 {
                state = .done(FileInfo(length: 0, syncedBytes: 0, fullySync: true, sha256: "", text: nil))
                return
            }

            // Read the full file in chunks, computing SHA-256 along the way.
            let chunkSize: UInt64 = 65536
            var offset: UInt64 = 0
            var hasher = SHA256()
            var allData = Data(capacity: Int(min(length, 1024 * 1024)))

            while offset < length {
                let chunk = try await file.read(offset, min(chunkSize, length - offset))
                hasher.update(data: chunk)
                allData.append(chunk)
                offset += UInt64(chunk.count)
                state = .reading(Double(offset) / Double(length))
            }

            let digest = hasher.finalize()
            let hex = digest.map { String(format: "%02x", $0) }.joined()
            let text = allData.count <= 4096 ? String(data: allData, encoding: .utf8) : nil

            state = .done(FileInfo(length: length, syncedBytes: length, fullySync: true, sha256: hex, text: text))
        } catch {
            state = .error(error.localizedDescription)
        }
    }

    // MARK: - Write

    private func writeContent(_ text: String) async {
        guard let repo else { return }
        do {
            let data = Data(text.utf8)
            let file = try await repo.createFile(path)
            defer { Task { try? await file.close() } }
            try await file.write(0, data)
            try await file.flush()
            await loadFile()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

// MARK: - Write-content sheet

private struct WriteContentSheet: View {
    let onSubmit: (String) -> Void
    let onCancel: () -> Void

    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Write content").font(.headline)
            TextEditor(text: $text)
                .font(.system(.body, design: .monospaced))
                .frame(width: 380, height: 160)
                .border(Color.secondary.opacity(0.3))
            HStack {
                Spacer()
                Button("Cancel") { onCancel() }
                Button("Write") { onSubmit(text) }
                    .buttonStyle(.borderedProminent)
                    .disabled(text.isEmpty)
            }
        }
        .frame(width: 400)
    }
}

// MARK: - Supporting types

private enum FileState {
    case loading
    case syncing(Double)
    case reading(Double)
    case done(FileInfo)
    case error(String)
}

private struct FileInfo {
    let length: UInt64
    let syncedBytes: UInt64
    let fullySync: Bool
    let sha256: String
    let text: String?       // non-nil when file is small and valid UTF-8
}

// MARK: - Helpers

private func formatSize(_ bytes: UInt64) -> String {
    let kilo: UInt64 = 1024
    let mega: UInt64 = 1024 * 1024
    let giga: UInt64 = 1024 * 1024 * 1024

    if bytes >= giga { return String(format: "%.1f GiB", Double(bytes) / Double(giga)) }
    if bytes >= mega { return String(format: "%.1f MiB", Double(bytes) / Double(mega)) }
    if bytes >= kilo { return String(format: "%.1f KiB", Double(bytes) / Double(kilo)) }
    return "\(bytes) B"
}
