// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "OuisyncExample",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(path: "../OuisyncLib"),
    ],
    targets: [
        .executableTarget(
            name: "OuisyncExample",
            dependencies: [
                .product(name: "OuisyncLib", package: "OuisyncLib"),
            ],
            path: "Sources/OuisyncExample"
        ),
    ]
)
