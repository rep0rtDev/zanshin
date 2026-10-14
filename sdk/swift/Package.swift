// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Zanshin",
    platforms: [.iOS(.v15), .macOS(.v12), .tvOS(.v15), .watchOS(.v8), .visionOS(.v1)],
    products: [
        .library(name: "Zanshin", targets: ["Zanshin"]),
    ],
    targets: [
        .target(name: "Zanshin", path: "Sources/Zanshin"),
        .testTarget(name: "ZanshinTests", dependencies: ["Zanshin"], path: "Tests/ZanshinTests"),
    ]
)
