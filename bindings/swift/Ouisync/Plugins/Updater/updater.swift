/* Swift package manager command plugin: invokes `update.sh` to download and compile rust deps.
 *
 * Because the companion build plugin cannot access the network, this plugin must be run every time
 * either `Cargo.toml` or `Cargo.lock` is updated, or the next build will fail.
 *
 * Can be run from Xcode by right clicking on the "Ouisync" package and picking
 * "Update rust dependencies" or directly via the command line:
 * `swift package plugin cargo-fetch --allow-network-connections all`.
 *
 * After a fresh `git clone` (or `git clean` or `flutter clean` or after using the
 * `Product > Clear Build Folder` menu action in Xcode, the `init` shell script from the swift
 * package root MUST be run before attempting a new build (it will run this script as well) */
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
