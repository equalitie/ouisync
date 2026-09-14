import SwiftUI
import UniformTypeIdentifiers
import CryptoKit
import OuisyncLib

struct FileView: View {
    @EnvironmentObject private var viewModel: ExampleViewModel
    let repositoryName: String
    let path: String

    @State private var state: FileState = .loading
    @State private var isExporting = false
    @State private var exportDocument: ExportedFile?
    @State private var exportProgress: Double?
    @State private var errorMessage: String?

    private var fileName: String { (path as NSString).lastPathComponent }

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
        .navigationTitle(fileName)
        .task { await loadFile() }
        .overlay { if let exportProgress { exportOverlay(exportProgress) } }
        .toolbar {
            ToolbarItem {
                Button { Task { await prepareExport() } } label: { Image(systemName: "square.and.arrow.down") }
                    .help("Export / download this file")
                    .disabled(exportProgress != nil)
            }
            ToolbarItem {
                Button { Task { await loadFile() } } label: { Image(systemName: "arrow.clockwise") }
                    .help("Refresh")
            }
        }
        .fileExporter(
            isPresented: $isExporting,
            document: exportDocument,
            contentType: .data,
            defaultFilename: fileName
        ) { result in
            if case .failure(let error) = result {
                errorMessage = error.localizedDescription
            }
            exportDocument?.cleanup()
            exportDocument = nil
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

    // MARK: - Export

    private func exportOverlay(_ fraction: Double) -> some View {
        ZStack {
            Color.black.opacity(0.2).ignoresSafeArea()
            VStack(spacing: 12) {
                ProgressView(value: fraction) { Text("Preparing \(fileName)…") }
                Text("\(Int(fraction * 100))%").foregroundStyle(.secondary)
            }
            .padding(24)
            .frame(maxWidth: 320)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
        }
    }

    /// Streams the repository file into a temporary file on disk, updating
    /// `exportProgress` as it goes. The returned URL lives in a unique temp
    /// subdirectory. Throws (and cleans up) on failure.
    private func downloadToTemp() async throws -> URL {
        guard let repo else { throw CocoaError(.fileNoSuchFile) }

        let tempURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent(fileName)

        do {
            try FileManager.default.createDirectory(
                at: tempURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            FileManager.default.createFile(atPath: tempURL.path, contents: nil)

            let output = try FileHandle(forWritingTo: tempURL)
            defer { try? output.close() }

            let file = try await repo.openFile(path)
            defer { Task { try? await file.close() } }

            let length = try await file.getLength()
            let chunkSize: UInt64 = 65536
            var offset: UInt64 = 0

            while offset < length {
                let chunk = try await file.read(offset, min(chunkSize, length - offset))
                if chunk.isEmpty { break }
                try output.write(contentsOf: chunk)
                offset += UInt64(chunk.count)
                exportProgress = length > 0 ? Double(offset) / Double(length) : 1.0
            }

            return tempURL
        } catch {
            try? FileManager.default.removeItem(at: tempURL.deletingLastPathComponent())
            throw error
        }
    }

    /// Downloads the file to a temp location and presents the system exporter
    /// so the user can save it wherever they like.
    private func prepareExport() async {
        exportProgress = 0
        do {
            let tempURL = try await downloadToTemp()
            exportProgress = nil
            exportDocument = ExportedFile(url: tempURL)
            isExporting = true
        } catch {
            exportProgress = nil
            errorMessage = "Failed to export \(fileName): \(error.localizedDescription)"
        }
    }
}

// MARK: - Exported file document

/// Wraps an on-disk temporary file for use with `.fileExporter`. The exporter
/// copies it to the user-chosen destination without loading it into memory.
private struct ExportedFile: FileDocument {
    static var readableContentTypes: [UTType] { [.data] }

    let url: URL

    init(url: URL) { self.url = url }

    init(configuration: ReadConfiguration) throws {
        throw CocoaError(.fileReadUnsupportedScheme)
    }

    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper {
        try FileWrapper(url: url)
    }

    /// Removes the temporary directory backing this document.
    func cleanup() {
        try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
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
