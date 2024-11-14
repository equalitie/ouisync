/* Swift package manager build plugin: currently invokes `Builder` before every build.


 Ideally, a `.buildTool()`[1] plugin[2][3][4] is expected to provide makefile-like rules mapping
 supplied files to their requirements, which are then used by the build system to only compile the
 necessary targets when they are needed upstream and their inputs have changed.

 While this would be sufficient when coupled with re-running the builder plugin (e.g. re-reading
 the makefile) on every build, certain acknowledged[5] missing features[6] currently prevent us
 from using `.buildTool()` as intended. Instead, we currently opt for `.prebuildTool()` and mostly
 rely on `cargo` to compile incrementally while avoiding unnecessary work.

 [1] https://developer.apple.com/documentation/packagedescription/target/plugincapability-swift.enum/buildtool())
 [2] https://github.com/swiftlang/swift-package-manager/blob/main/Documentation/Plugins.md
 [3] https://github.com/swiftlang/swift-evolution/blob/main/proposals/0303-swiftpm-extensible-build-tools.md
 [4] https://github.com/swiftlang/swift-evolution/blob/main/proposals/0325-swiftpm-additional-plugin-apis.md
 [5] https://forums.swift.org/t/swiftpm-trouble-accessing-non-swift-buildtool-artifacts-in-derived-data-path/64444
 [6] https://forums.swift.org/t/build-dependent-packages-automatically/69648
 */
import Foundation
import PackagePlugin

@main struct TreeReconciler: BuildToolPlugin {
    func panic(_ msg: String) -> Never {
        Diagnostics.error(msg)
        fatalError("Unable to build LibOuisyncFFI.xcframework")
    }

    func createBuildCommands(context: PackagePlugin.PluginContext,
                             target: PackagePlugin.Target) async throws -> [PackagePlugin.Command] {
        let output = context.pluginWorkDirectory

        let dependencies = output
            .removingLastComponent() // FFIBuilder
            .removingLastComponent() // OuisyncLibFFI
            .removingLastComponent() // ouisync.output
            .appending("Update rust dependencies.output")

        guard FileManager.default.fileExists(atPath: dependencies.string)
        else { panic("Please run `Update rust dependencies` on the OuisyncLib package") }

        let manifest = context.package.directory
            .removingLastComponent() // OuisyncLib
            .removingLastComponent() // swift
            .removingLastComponent() // bindings
            .appending("Cargo.toml")

        return [.prebuildCommand(displayName: "Build OuisyncLibFFI.xcframework",
                                 executable: context.package.directory.appending("Plugins").appending("build.sh"),
                                 arguments: [which("cargo"),
                                             which("rustc"),
                                             manifest.string,
                                             output.string,
                                             dependencies],
                                 environment: ProcessInfo.processInfo.environment,
                                 outputFilesDirectory: output.appending("dummy"))]
    }

    // runs `which binary` in the default shell and returns the path after confirming that it exists
    private func which(_ binary: String) -> String {
        let path = shell("which \(binary)").trimmingCharacters(in: .whitespacesAndNewlines)

        guard FileManager.default.fileExists(atPath: path)
        else { panic("Unable to find `\(binary)` in environment.") }

        Diagnostics.remark("Found `\(binary)` at \(path)")
        return path
    }

    // runs `command` in the default shell and returns stdout on clean exit; throws otherwise
    private func shell(_ command: String) -> String {
        exec(command: ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh",
             with: ["-c", command])
    }

    // runs `exe` using `args` in `env` and returns `stdout`; panics on non-zero exit
    @discardableResult private func exec(command exe: String,
                                         with args: [String] = [],
                                         in cwd: String? = nil,
                                         using env: [String: String]? = nil) -> String {
        let pipe = Pipe()
        let task = Process()
        task.standardInput = nil
        task.standardOutput = pipe
        task.executableURL = URL(fileURLWithPath: exe)
        task.arguments = args
        if let env { task.environment = env }
        if let cwd { task.currentDirectoryURL = URL(fileURLWithPath: cwd) }

        do { try task.run() } catch { panic("Unable to start \(exe): \(error)") }
        var stdout: Data?
        do { stdout = try pipe.fileHandleForReading.readToEnd() }
        catch { Diagnostics.warning(String(describing: error)) }
        task.waitUntilExit()

        guard task.terminationReason ~= .exit else { panic("\(exe) killed by \(task.terminationStatus)") }
        guard task.terminationStatus == 0 else { panic("\(exe) returned \(task.terminationStatus)") }
        guard let res = String(data: stdout ?? Data(), encoding: .utf8) else { panic("\(exe) produced binary data") }

        return res
    }
}
