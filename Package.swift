// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "BarkBoltFFI",
    platforms: [
        .iOS(.v16),
        .macOS(.v13),
    ],
    products: [
        .library(
            name: "BarkBoltFFI",
            targets: ["BarkBoltFFI"]
        ),
    ],
    targets: [
        .binaryTarget(
            name: "BarkBoltFFIFFIBinary",
            url: "https://github.com/smolcars/bark-bolt-ffi/releases/download/v0.7.2/BarkBoltFFI.xcframework.zip", checksum: "3ebde88fa92b1a8d900c67dacb6383844f75b7356cb5c097906697326b1b50c6"
        ),
        .target(
            name: "BarkBoltFFIFFI",
            dependencies: ["BarkBoltFFIFFIBinary"],
            path: "sdk/swift/FFI",
            publicHeadersPath: "include"
        ),
        .target(
            name: "BarkBoltFFI",
            dependencies: ["BarkBoltFFIFFI"],
            path: "sdk/swift/Sources"
        ),
    ]
)
