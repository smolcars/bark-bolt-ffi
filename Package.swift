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
            url: "https://github.com/smolcars/bark-bolt-ffi/releases/download/v0.9.0/BarkBoltFFI.xcframework.zip", checksum: "2ba7ed45d5fb6db2983dbe0039611c30f9ac9edb8a31e989c44abf998bfa0e35"
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
