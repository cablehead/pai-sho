// swift-tools-version: 6.0
import PackageDescription

// The screens the iOS and Mac apps share: the daemon client, the peer list,
// the add-a-peer sheet, the QR scanner and the page view. Each app links this
// and adds only what differs, such as running the process in the app on iOS.
let package = Package(
    name: "PaiShoKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "PaiShoKit", targets: ["PaiShoKit"])
    ],
    targets: [
        .target(name: "PaiShoKit"),
        .testTarget(name: "PaiShoKitTests", dependencies: ["PaiShoKit"]),
    ]
)
