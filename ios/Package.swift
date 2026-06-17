// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "tauri-plugin-cblite",
    platforms: [
        // Xcode 26 PackageDescription dropped constants below macOS 14 / iOS 15.
        // (Also satisfies CouchbaseLiteSwift 4.0.4's macOS 13+ / iOS 13+ floor.)
        .macOS(.v14),
        .iOS(.v15),
    ],
    products: [
        .library(
            name: "tauri-plugin-cblite",
            type: .static,
            targets: ["tauri-plugin-cblite"]),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
        .package(name: "CouchbaseLiteSwift",
                 url: "https://github.com/couchbase/couchbase-lite-swift.git",
                 .upToNextMajor(from: "4.0.4")),
    ],
    targets: [
        .target(
            name: "tauri-plugin-cblite",
            dependencies: [
                .byName(name: "Tauri"),
                .product(name: "CouchbaseLiteSwift", package: "CouchbaseLiteSwift"),
            ],
            path: "Sources")
    ]
)
