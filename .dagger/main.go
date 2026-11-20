package main

import (
	"context"
	"dagger/pai-sho/internal/dagger"
)

type PaiSho struct{}

func (m *PaiSho) withCaches(container *dagger.Container, targetSuffix string) *dagger.Container {
	// Separate caches per target
	registryCache := dag.CacheVolume("dagger-cargo-registry-" + targetSuffix)
	gitCache := dag.CacheVolume("dagger-cargo-git-" + targetSuffix)
	targetCache := dag.CacheVolume("dagger-cargo-target-" + targetSuffix)

	return container.
		WithMountedCache("/root/.cargo/registry", registryCache).
		WithMountedCache("/root/.cargo/git", gitCache).
		WithMountedCache("/app/target", targetCache)
}

func (m *PaiSho) Upload(
	ctx context.Context,
	// +ignore=["**", "!Cargo.toml", "!Cargo.lock", "!src/**"]
	src *dagger.Directory) *dagger.Directory {
	return src
}

func (m *PaiSho) MacosEnv(
	ctx context.Context,
	src *dagger.Directory) *dagger.Container {
	return m.withCaches(
		dag.Container().
			From("joseluisq/rust-linux-darwin-builder:latest").
			WithEnvVariable("CC_aarch64_apple_darwin", "aarch64-apple-darwin22.4-clang").
			WithEnvVariable("CXX_aarch64_apple_darwin", "aarch64-apple-darwin22.4-clang++").
			WithEnvVariable("AR_aarch64_apple_darwin", "aarch64-apple-darwin22.4-ar").
			WithEnvVariable("CFLAGS_aarch64_apple_darwin", "-fuse-ld=/usr/local/osxcross/target/bin/aarch64-apple-darwin22.4-ld").
			WithMountedDirectory("/app", src).
			WithWorkdir("/app"),
		"darwin-arm64",
	)
}

func (m *PaiSho) MacosBuild(ctx context.Context, src *dagger.Directory, version string) *dagger.File {
	container := m.MacosEnv(ctx, src).
		WithExec([]string{"rustup", "update", "stable"}).
		WithExec([]string{"rustup", "default", "stable"}).
		WithExec([]string{"rustup", "target", "add", "aarch64-apple-darwin"})

	// First build attempt - this will likely fail due to libproc issue
	container = container.WithExec([]string{"bash", "-c", `
		cargo build --target aarch64-apple-darwin --release --color always 2>&1 | tee build.log
		
		# Check if libproc error occurred
		if grep -q "osx_libproc_bindings.rs.*No such file" build.log; then
			echo "Detected libproc issue, applying fix..."
			
			# Find the libproc source file - try both possible paths
			SOURCE_FILE=$(find /root/.cargo/registry/src/index.crates.io-* -name "libproc-*" -type d | head -1)/docs_rs/osx_libproc_bindings.rs
			if [ ! -f "$SOURCE_FILE" ]; then
				SOURCE_FILE=$(find /root/.cargo/registry/src/index.crates.io-* -name "libproc-*" -type d | head -1)/src/osx_libproc_bindings.rs
			fi
			
			# Find the destination directory
			DEST_DIR=$(find target/aarch64-apple-darwin/release/build/ -name "libproc-*" -type d | head -1)/out
			
			if [ -f "$SOURCE_FILE" ] && [ -d "$DEST_DIR" ]; then
				echo "Copying $SOURCE_FILE to $DEST_DIR/"
				cp "$SOURCE_FILE" "$DEST_DIR/"
				
				echo "Retrying build..."
				cargo build --target aarch64-apple-darwin --release --color always
			else
				echo "Error: Could not find source file or destination directory"
				echo "Source: $SOURCE_FILE"
				echo "Dest: $DEST_DIR"
				exit 1
			fi
		fi
		
		# Clean up log file
		rm -f build.log
	`})

	// Create tarball structure using provided version
	container = container.WithExec([]string{"sh", "-c", `
		mkdir -p /tmp/pai-sho-` + version + `
		cp target/aarch64-apple-darwin/release/pai-sho /tmp/pai-sho-` + version + `/
		cd /tmp
		tar -czf pai-sho-` + version + `-macos-arm64.tar.gz pai-sho-` + version + `
	`})

	return container.File("/tmp/pai-sho-" + version + "-macos-arm64.tar.gz")
}

func (m *PaiSho) WindowsEnv(
	ctx context.Context,
	src *dagger.Directory) *dagger.Container {
	return m.withCaches(
		dag.Container().
			From("joseluisq/rust-linux-darwin-builder:latest").
			WithExec([]string{"apt", "update"}).
			WithExec([]string{"apt", "install", "-y", "gcc-mingw-w64-x86-64"}).
			WithEnvVariable("CC_x86_64_pc_windows_gnu", "x86_64-w64-mingw32-gcc").
			WithEnvVariable("AR_x86_64_pc_windows_gnu", "x86_64-w64-mingw32-gcc-ar").
			WithMountedDirectory("/app", src).
			WithWorkdir("/app"),
		"windows-amd64",
	)
}

func (m *PaiSho) WindowsBuild(ctx context.Context, src *dagger.Directory, version string) *dagger.File {
	container := m.WindowsEnv(ctx, src).
		WithExec([]string{"rustup", "update", "stable"}).
		WithExec([]string{"rustup", "default", "stable"}).
		WithExec([]string{"rustup", "target", "add", "x86_64-pc-windows-gnu"}).
		WithExec([]string{"cargo", "build", "--release", "--target", "x86_64-pc-windows-gnu"})

	container = container.WithExec([]string{"sh", "-c", `
		set -e
		mkdir -p /tmp/pai-sho-` + version + `
		cp target/x86_64-pc-windows-gnu/release/pai-sho.exe /tmp/pai-sho-` + version + `/
		cd /tmp
		tar -czf pai-sho-` + version + `-windows-amd64.tar.gz pai-sho-` + version + `
	`})

	return container.File("/tmp/pai-sho-" + version + "-windows-amd64.tar.gz")
}

func (m *PaiSho) LinuxArm64Env(
	ctx context.Context,
	src *dagger.Directory) *dagger.Container {
	return m.withCaches(
		dag.Container().
			From("messense/rust-musl-cross:aarch64-musl").
			WithMountedDirectory("/app", src).
			WithWorkdir("/app"),
		"linux-arm64",
	)
}

func (m *PaiSho) LinuxArm64Build(ctx context.Context, src *dagger.Directory, version string) *dagger.File {
	container := m.LinuxArm64Env(ctx, src).
		WithExec([]string{"cargo", "build", "--release", "--target", "aarch64-unknown-linux-musl"})

	// Create tarball structure using provided version
	container = container.WithExec([]string{"sh", "-c", `
		mkdir -p /tmp/pai-sho-` + version + `
		cp target/aarch64-unknown-linux-musl/release/pai-sho /tmp/pai-sho-` + version + `/
		cd /tmp
		tar -czf pai-sho-` + version + `-linux-arm64.tar.gz pai-sho-` + version + `
	`})

	return container.File("/tmp/pai-sho-" + version + "-linux-arm64.tar.gz")
}

func (m *PaiSho) LinuxAmd64Env(
	ctx context.Context,
	src *dagger.Directory) *dagger.Container {
	return m.withCaches(
		dag.Container().
			From("messense/rust-musl-cross:x86_64-musl").
			WithMountedDirectory("/app", src).
			WithWorkdir("/app"),
		"linux-amd64",
	)
}

func (m *PaiSho) LinuxAmd64Build(ctx context.Context, src *dagger.Directory, version string) *dagger.File {
	container := m.LinuxAmd64Env(ctx, src).
		WithExec([]string{"cargo", "build", "--release", "--target", "x86_64-unknown-linux-musl"})

	// Create tarball structure using provided version
	container = container.WithExec([]string{"sh", "-c", `
		mkdir -p /tmp/pai-sho-` + version + `
		cp target/x86_64-unknown-linux-musl/release/pai-sho /tmp/pai-sho-` + version + `/
		cd /tmp
		tar -czf pai-sho-` + version + `-linux-amd64.tar.gz pai-sho-` + version + `
	`})

	return container.File("/tmp/pai-sho-" + version + "-linux-amd64.tar.gz")
}
