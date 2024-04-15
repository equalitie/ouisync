// swift-tools-version: 5.10
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let package = Package(
    name: "OuisyncLib",
    platforms: [.macOS(.v13), .iOS(.v13), .macCatalyst(.v13)],
    products: [
        // Products define the executables and libraries a package produces, making them visible to other packages.
        .library(
            name: "OuisyncLib",
            targets: ["OuisyncLib"]),
    ],
    dependencies: [
        .package(url: "https://github.com/a2/MessagePack.swift.git", from: "4.0.0")
    ],
    targets: [
        // Targets are the basic building blocks of a package, defining a module or a test suite.
        // Targets can depend on other targets in this package and products from dependencies.
        .target(
            name: "OuisyncLib",
            dependencies: [.product(name:"MessagePack", package: "MessagePack.swift")]),
        .testTarget(
            name: "OuisyncLibTests",
            dependencies: ["OuisyncLib"]),
    ]
)
