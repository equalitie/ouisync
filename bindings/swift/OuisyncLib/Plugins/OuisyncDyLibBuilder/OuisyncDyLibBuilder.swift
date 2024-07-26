//
//  File.swift
//  
//
//  Created by Peter Jankuliak on 26/07/2024.
//

// A package plugin which builds the libouisync_ffi.dylib library and includes it in the package.
// https://github.com/swiftlang/swift-package-manager/blob/main/Documentation/Plugins.md
//
// TODO: Right now it creates only Debug version of the library and only for the current architecture.
// Can we detect the build type and architecture here? This github ticket seems to indicate it's not
// currently possible:
// https://github.com/swiftlang/swift-package-manager/issues/7110

import Foundation
import PackagePlugin

@main
struct OuisyncDyLibBuilder: BuildToolPlugin {
    func createBuildCommands(context: PackagePlugin.PluginContext, target: PackagePlugin.Target) async throws -> [PackagePlugin.Command] {
        let workDir = context.pluginWorkDirectory
        let dylibName = "libouisync_ffi.dylib"
        let dylibPath = workDir.appending("debug").appending(dylibName)
        let cargoPath = shell("which cargo").trimmingCharacters(in: .whitespacesAndNewlines)
        let inputFiles = findInputFiles()

        return [
            .buildCommand(
                displayName: "Build Ouisync FFI .dylib",
                executable: Path(cargoPath),
                arguments: ["build", "-p", "ouisync-ffi", "--target-dir", workDir],
                environment: [:],
                inputFiles: inputFiles.map { Path($0) },
                outputFiles: [ dylibPath ])
        ]
    }
}

// This finds files which when changed, the Swift builder will re-execute the build above build command.
// The implementation is not very good because if a new directory/module is added on which this package
// depends, the package builder won't rebuild the ffi library.
// TODO: use `cargo build --build-plan` once it's in stable.
func findInputFiles() -> [String] {
    var files = [String]()
    let startAts = [
        URL(string: "../../../bridge")!,
        URL(string: "../../../deadlock")!,
        URL(string: "../../../ffi")!,
        URL(string: "../../../lib")!,
        URL(string: "../../../net")!,
        URL(string: "../../../rand")!,
        URL(string: "../../../scoped_task")!,
        URL(string: "../../../state_monitor")!,
        URL(string: "../../../tracing_fmt")!,
        URL(string: "../../../utils")!,
        URL(string: "../../../vfs")!,
    ]
    for startAt in startAts {
        if let enumerator = FileManager.default.enumerator(at: startAt, includingPropertiesForKeys: [.isRegularFileKey], options: [.skipsHiddenFiles, .skipsPackageDescendants]) {
            for case let fileURL as URL in enumerator {
                do {
                    let fileAttributes = try fileURL.resourceValues(forKeys:[.isRegularFileKey])
                    if fileAttributes.isRegularFile! {
                        files.append(fileURL.path)
                    }
                } catch { fatalError("Error finding input files: \(error), \(fileURL)") }
            }
        }
    }
    return files
}

func shell(_ command: String) -> String {
    let task = Process()
    let pipe = Pipe()

    task.standardOutput = pipe
    task.standardError = pipe
    task.arguments = ["-c", command]
    task.launchPath = "/bin/zsh"
    task.standardInput = nil
    task.launch()

    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    let output = String(data: data, encoding: .utf8)!

    return output
}
