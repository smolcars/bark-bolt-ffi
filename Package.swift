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
            url: "https://github.com/smolcars/bark-bolt-ffi/releases/download/v0.8.0/BarkBoltFFI.xcframework.zip", checksum: "bcadced292d63df92cad7f5e02ba6ce2037b2c0304edb1e680fe7e48559d26c2"
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
