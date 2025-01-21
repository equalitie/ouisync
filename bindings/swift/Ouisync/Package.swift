// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Ouisync",
    platforms: [.macOS(.v13), .iOS(.v16)],
    products: [
        .library(name: "Ouisync",
                 type: .static,
                 targets: ["Ouisync"]),
    ],
    dependencies: [
        .package(url: "https://github.com/a2/MessagePack.swift.git", from: "4.0.0"),
    ],
    targets: [
        .target(name: "Ouisync",
                dependencies: [.product(name: "MessagePack",
                                        package: "MessagePack.swift"),
                               "CargoBuild",
                               "OuisyncService"],
                path: "Sources"),
        .testTarget(name: "OuisyncTests",
                    dependencies: ["Ouisync"],
                    path: "Tests"),
        .binaryTarget(name: "OuisyncService",
                      path: "OuisyncService.xcframework"),
        .plugin(name: "CargoBuild",
                capability: .buildTool(),
                path: "Plugins/Builder"),
        .plugin(name: "Update rust dependencies",
                capability: .command(intent: .custom(verb: "cargo-fetch",
                                                     description: "Update rust dependencies"),
                                     permissions: [
                .allowNetworkConnections(scope: .all(),
                                         reason: "Downloads dependencies defined by Cargo.toml")]),
                path: "Plugins/Updater"),
    ]
)
