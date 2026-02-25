.PHONY: dist

CROSS := .cargo/bin/cross

# Compile the binaries for all targets.
build: \
	build-x86_64-unknown-linux-musl \
	build-aarch64-unknown-linux-musl \
	build-armv5te-unknown-linux-musleabi \
	build-armv7-unknown-linux-musleabihf \
	build-mipsel-unknown-linux-musl

build-x86_64-unknown-linux-musl:
	$(CROSS) build --target x86_64-unknown-linux-musl --release

build-aarch64-unknown-linux-musl:
	$(CROSS) build --target aarch64-unknown-linux-musl --release

build-armv5te-unknown-linux-musleabi:
	$(CROSS) build --target armv5te-unknown-linux-musleabi --release

build-armv7-unknown-linux-musleabihf:
	$(CROSS) build --target armv7-unknown-linux-musleabihf --release

# Build distributable binaries for all targets.
dist: \
	dist-x86_64-unknown-linux-musl \
	dist-aarch64-unknown-linux-musl \
	dist-armv5te-unknown-linux-musleabi \
	dist-armv7-unknown-linux-musleabihf \
	dist-mipsel-unknown-linux-musl

dist-x86_64-unknown-linux-musl: build-x86_64-unknown-linux-musl package-x86_64-unknown-linux-musl

dist-aarch64-unknown-linux-musl: build-aarch64-unknown-linux-musl package-aarch64-unknown-linux-musl

dist-armv5te-unknown-linux-musleabi: build-armv5te-unknown-linux-musleabi package-armv5te-unknown-linux-musleabi

dist-armv7-unknown-linux-musleabihf: build-armv7-unknown-linux-musleabihf package-armv7-unknown-linux-musleabihf

dist-mipsel-unknown-linux-musl: build-mipsel-unknown-linux-musl package-mipsel-unknown-linux-musl

# Package the compiled binaries
package-x86_64-unknown-linux-musl:
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist

	# .tar.gz
	tar -czvf dist/rak-basicstation_$(PKG_VERSION)_amd64.tar.gz -C target/x86_64-unknown-linux-musl/release rak-basicstation

	# .deb
	cargo deb --target x86_64-unknown-linux-musl --no-build --no-strip
	cp target/x86_64-unknown-linux-musl/debian/*.deb ./dist

package-aarch64-unknown-linux-musl:
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist

	# .tar.gz
	tar -czvf dist/rak-basicstation_$(PKG_VERSION)_arm64.tar.gz -C target/aarch64-unknown-linux-musl/release rak-basicstation

	# .deb
	cargo deb --target aarch64-unknown-linux-musl --no-build --no-strip
	cp target/aarch64-unknown-linux-musl/debian/*.deb ./dist

package-armv7-unknown-linux-musleabihf:
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist

	# .tar.gz
	tar -czvf dist/rak-basicstation_$(PKG_VERSION)_armv7hf.tar.gz -C target/armv7-unknown-linux-musleabihf/release rak-basicstation

	# .deb
	cargo deb --target armv7-unknown-linux-musleabihf --no-build --no-strip
	cp target/armv7-unknown-linux-musleabihf/debian/*.deb ./dist

package-armv5te-unknown-linux-musleabi:
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist

	# .tar.gz
	tar -czvf dist/rak-basicstation_$(PKG_VERSION)_armv5te.tar.gz -C target/armv5te-unknown-linux-musleabi/release rak-basicstation

	# .deb
	cargo deb --target armv5te-unknown-linux-musleabi --no-build --no-strip
	cp target/armv5te-unknown-linux-musleabi/debian/*.deb ./dist

package-mipsel-unknown-linux-musl: package-rak-mipsel_24kc

package-rak-mipsel_24kc:
	cd packaging/vendor/rak/mipsel_24kc && ./package.sh
	mkdir -p dist/vendor/rak/mipsel_24kc
	cp packaging/vendor/rak/mipsel_24kc/*.ipk dist/vendor/rak/mipsel_24kc

build-mipsel-unknown-linux-musl:
	# mipsel is a tier-3 target.
	rustup toolchain add nightly-2026-01-27-x86_64-unknown-linux-gnu
	$(CROSS) +nightly-2026-01-27 build -Z build-std=panic_abort,std --target mipsel-unknown-linux-musl --release --no-default-features --features semtech_udp

# Install pinned build dependencies (run once before cross-compiling).
dev-dependencies:
	cargo install cross --git https://github.com/cross-rs/cross --rev 452dc27a11d4f58d65309f8455c5cf7558f60513 --locked --root .cargo

# Update the version.
version:
	test -n "$(VERSION)"
	sed -i 's/^  version.*/  version = "$(VERSION)"/g' ./Cargo.toml
	make test
	git add .
	git commit -v -m "Bump version to $(VERSION)"
	git tag -a v$(VERSION) -m "v$(VERSION)"

# Cleanup dist.
clean:
	cargo clean
	rm -rf dist

# Run tests
test:
	cargo clippy --no-deps
	cargo test

