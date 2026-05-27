// swift-tools-version: 5.9
import PackageDescription


let package = Package(
    name: "OuisyncLib",
    platforms: [.macOS(.v13), .iOS(.v16)],
    products: [
        .library(name: "OuisyncLib",
                 type: .static,
                 targets: ["OuisyncLib"]),
        .library(name: "OuisyncLibCore",
                 type: .static,
                 targets: ["OuisyncLibCore"]),
    ],
    dependencies: [
        .package(url: "https://github.com/a2/MessagePack.swift.git", from: "4.0.0"),
    ],
    targets: [
        .target(name: "OuisyncLibCore",
                dependencies: [.product(name: "MessagePack",
                                        package: "MessagePack.swift")],
                path: "Sources"),
        .target(name: "OuisyncLib",
                dependencies: ["OuisyncLibCore",
                               "FFIBuilder",
                               "OuisyncLibFFI"],
                path: "SourcesFFI"),
        .testTarget(name: "OuisyncLibTests",
                    dependencies: ["OuisyncLibCore"],
                    path: "Tests"),
        // FIXME: move this to a separate package / framework
        .binaryTarget(name: "OuisyncLibFFI",
                      path: "output/OuisyncLibFFI.xcframework"),
        .plugin(name: "FFIBuilder",
                capability: .buildTool(),
                path: "Plugins/Builder"),
        .plugin(name: "Update rust dependencies",
                capability: .command(intent: .custom(verb: "cargo-fetch",
                                                     description: "Update rust dependencies"),
                                     permissions: [
                .allowNetworkConnections(scope: .all(),
                                         reason: "Downloads dependencies defined by Cargo.toml"),
                .writeToPackageDirectory(reason: "These are not the droids you are looking for")]),
                path: "Plugins/Updater"),
    ]
)
