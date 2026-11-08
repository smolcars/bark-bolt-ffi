{
  description = "Pinned Android build tools for Bark's Linux CI runners";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        config = {
          allowUnfree = true;
          android_sdk.accept_license = true;
        };
      };
      android = pkgs.androidenv.composeAndroidPackages {
        platformVersions = [ "35" ];
        buildToolsVersions = [ "35.0.0" ];
        includeNDK = true;
        ndkVersions = [ "28.2.13676358" ];
        includeEmulator = false;
        includeSystemImages = false;
      };
      sdk = "${android.androidsdk}/libexec/android-sdk";
      boltffi = pkgs.stdenvNoCC.mkDerivation {
        pname = "boltffi";
        version = "0.31.1";
        src = pkgs.fetchurl {
          url = "https://github.com/boltffi/boltffi/releases/download/v0.31.1/boltffi-linux-x86_64.tar.gz";
          sha256 = "ae02515544f6553e72b2fd5ea99ed604b047d163dfb7fa2e1534dda92756cc28";
        };
        nativeBuildInputs = [ pkgs.autoPatchelfHook ];
        buildInputs = [ pkgs.stdenv.cc.cc.lib ];
        dontConfigure = true;
        dontBuild = true;
        unpackPhase = ''tar -xzf "$src"'';
        installPhase = ''install -Dm755 boltffi "$out/bin/boltffi"'';
      };
    in {
      devShells.${system}.android = pkgs.mkShellNoCC {
        packages = with pkgs; [
          bash git curl unzip python3 just jq cmake pkg-config protobuf
          rustup jdk17 android.androidsdk boltffi
        ];
        JAVA_HOME = pkgs.jdk17.home;
        ANDROID_HOME = sdk;
        ANDROID_SDK_ROOT = sdk;
        ANDROID_NDK_HOME = "${sdk}/ndk-bundle";
        ANDROID_NDK_ROOT = "${sdk}/ndk-bundle";
        # Host-linked JNI and Rust tests must also run on Ubuntu without Nix.
        CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = "/usr/bin/cc";
        CC_x86_64_unknown_linux_gnu = "/usr/bin/cc";
        CXX_x86_64_unknown_linux_gnu = "/usr/bin/c++";
      };
    };
}
