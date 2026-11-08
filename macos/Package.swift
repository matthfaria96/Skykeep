// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Skykeep",
    platforms: [.macOS(.v26)],
    targets: [
        .executableTarget(
            name: "Skykeep",
            path: "Sources/Skykeep",
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                .linkedFramework("EventKit"), .linkedFramework("Contacts"), .linkedFramework("Photos"),
                .linkedFramework("QuickLookThumbnailing"), .linkedFramework("LocalAuthentication"),
                .linkedFramework("AVKit"), .linkedFramework("Security"),
            ]
        )
    ]
)
