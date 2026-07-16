// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "Autophagy",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "autophagy-app", targets: ["AutophagyApp"]),
        .library(name: "AutophagyKit", targets: ["AutophagyKit"])
    ],
    targets: [
        // Non-UI, fully testable core: read-only database access, schema
        // tolerance, model decoding, and CLI-command construction.
        .target(
            name: "AutophagyKit",
            linkerSettings: [
                .linkedLibrary("sqlite3")
            ]
        ),
        // SwiftUI shell. Kept deliberately thin: every non-trivial behaviour
        // lives in AutophagyKit so it can be unit-tested without a UI.
        .executableTarget(
            name: "AutophagyApp",
            dependencies: ["AutophagyKit"]
        ),
        .testTarget(
            name: "AutophagyKitTests",
            dependencies: ["AutophagyKit"]
        )
    ]
)
