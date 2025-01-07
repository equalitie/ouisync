/* Swift package manager command plugin: Used to download and compile any rust dependencies

 Due to apple's policies regarding plugin access, this must be run manually manually and granted
 permission to bypass the sandbox restrictions. Can be run by right clicking OuisyncLib from Xcode
 or directly from the command line via: `swift package cargo-fetch`.

 For automated tasks, the permissions can be automatically granted on invocation via the
 `--allow-network-connections` and `--allow-writing-to-package-directory` flags respectively or
 the sandbox can be disabled altogether via `--disable-sandbox` though the latter is untested. */
import Foundation
import PackagePlugin

@main struct Updater: CommandPlugin {
    func panic(_ msg: String) -> Never {
        Diagnostics.error(msg)
        fatalError("Unable to update rust dependencies")
    }

    func performCommand(context: PackagePlugin.PluginContext,
                        arguments: [String] = []) async throws {
        let task = Process()
        let exe = context.package.directory.appending(["Plugins", "update.sh"]).string
        task.standardInput = nil
        task.executableURL = URL(fileURLWithPath: exe)
        task.arguments = [context.pluginWorkDirectory.string]
        do { try task.run() } catch { panic("Unable to start \(exe): \(error)") }
        task.waitUntilExit()

        guard task.terminationReason ~= .exit else { panic("\(exe) killed by \(task.terminationStatus)") }
        guard task.terminationStatus == 0 else { panic("\(exe) returned \(task.terminationStatus)") }
        Diagnostics.remark("Dependencies up to date!")
    }
}
