/* Swift package manager command plugin: Used to download and compile any rust dependencies

 Due to apple's policies regarding plugin access, this must be run manually manually and granted
 permission to bypass the sandbox restrictions. Can be run by right clicking OuisyncLib from Xcode
 or directly from the command line via: `swift package cargo-fetch`.

 For automated tasks, the permissions can be automatically granted on invocation via the
 `--allow-network-connections` and `--allow-writing-to-package-directory` flags respectively or
 the sandbox can be disabled altogether via `--disable-sandbox` though the latter is untested. */
import Foundation
import PackagePlugin

@main struct UpdateDeps: CommandPlugin {
    func panic(_ msg: String) -> Never {
        Diagnostics.error(msg)
        fatalError("Unable to update rust dependencies")
    }

    func performCommand(context: PackagePlugin.PluginContext,
                        arguments: [String] = []) async throws {
        let package = context.package.directory
        let manifest = package
            .removingLastComponent() // OuisyncLib
            .removingLastComponent() // swift
            .removingLastComponent() // bindings
            .appending("Cargo.toml")

        // install package deps
        let cargo = which("cargo")
        let rust = which("rustc")
        let output = context.pluginWorkDirectory
        exec(command: cargo,
             with: ["fetch"],
             using: ["CARGO_HOME": output.string,  // package plugin sandbox forces us to write here
                     "CARGO_HTTP_CHECK_REVOKE": "false", // this fails in the sandbox, for "reasons"
                     "MANIFEST_PATH": manifest.string, // path to Cargo.toml, avoids having to chdir
                     "RUSTC": rust])  // without this cargo assumes $CARGO_HOME/bin/rustc

        // we also have to install cbindgen because running it automatically doesn't always work
        exec(command:cargo,
             with: ["install", "--force", "cbindgen"],
             using: ["CARGO_HOME": output.string,  // package plugin sandbox forces us to write here)
                     "CARGO_HTTP_CHECK_REVOKE": "false", // this fails in the sandbox, for "reasons"
                     "RUSTC": rust, // without this cargo assumes $CARGO_HOME/bin/rustc
                     "PATH": Path(which("cc")).removingLastComponent().string]) // (╯°□°）╯︵ ┻━┻
        Diagnostics.remark("Dependencies up to date in \(output)!")

        // link project workspace to output folder
        let dest = context.pluginWorkDirectory
            .removingLastComponent()
            .appending("\(context.package.id).output")
            .appending("OuisyncLib")
            .appending("FFIBuilder")
        let link = package.appending("output")

        // create stub framework in output folder
        exec(command: package.appending("reset-output.sh").string, in: dest.string)

        // replace link
        let fm = FileManager.default
        try? fm.removeItem(atPath: link.string)
        try fm.createSymbolicLink(atPath: link.string, withDestinationPath: dest.appending("output").string)

        // run a build if possible
        do {
            let res = try packageManager.build(PackageManager.BuildSubset.target("OuisyncLib"),
                                               parameters: .init())
            guard res.succeeded else {
                Diagnostics.warning(res.logText)
                throw NSError(domain: "Build failed", code: 1)
            }
        } catch {
            Diagnostics.warning("Unable to auto rebuild: \(error)")
        }
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
