#!/bin/sh
# Xcode runs this before compiling the app. It builds libpai_sho.a for the
# platform being built, into ../target/<triple>/<profile>, where the app's
# OTHER_LDFLAGS link it from.
set -eu

export PATH="$HOME/.cargo/bin:$PATH"

# Xcode exports SDKROOT for the iOS SDK. Cargo also compiles build scripts for
# this Mac, and those must not link against the iOS SDK.
unset SDKROOT

profile=debug
release=
if [ "$CONFIGURATION" = "Release" ]; then
    profile=release
    release=--release
fi

cd "$PROJECT_DIR/.."
cargo build --lib --target "$PAI_SHO_RUST_TARGET" $release
ls -l "target/$PAI_SHO_RUST_TARGET/$profile/libpai_sho.a"
