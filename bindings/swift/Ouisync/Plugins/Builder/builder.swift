/* Swift package manager build plugin: currently invokes `build.sh` before every build.

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

@main struct Builder: BuildToolPlugin {
    func createBuildCommands(context: PackagePlugin.PluginContext,
                             target: PackagePlugin.Target) async throws -> [PackagePlugin.Command] {
        [.prebuildCommand(displayName: "Build OuisyncService.xcframework",
                          executable: context.package.directory.appending(["Plugins", "build.sh"]),
                          arguments: [context.pluginWorkDirectory.string],
                          outputFilesDirectory: context.pluginWorkDirectory.appending("dummy"))]
    }
}
