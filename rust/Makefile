PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
APPDIR := $(PREFIX)/share/applications

CARGO ?= cargo

VERSION := $(shell grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
TARGET_TRIPLE ?= x86_64-linux
DIST_NAME := sam-$(VERSION)-$(TARGET_TRIPLE)
DIST_DIR := dist/$(DIST_NAME)

.PHONY: all build test check probe install uninstall clean dist dist-portable

all: build

build:
	$(CARGO) build --release

test:
	$(CARGO) test --workspace

check:
	$(CARGO) fmt --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) test --workspace

# Read-only verification that this machine's steamclient.so matches what the
# interop layer expects. Requires Steam to be running.
probe:
	$(CARGO) run --release -p sam-steam --example probe_abi

install: build
	install -Dm755 target/release/sam $(DESTDIR)$(BINDIR)/sam
	install -Dm644 packaging/sam.desktop $(DESTDIR)$(APPDIR)/sam.desktop
	@echo "Installed to $(BINDIR)/sam"
	@echo "Ensure $(BINDIR) is on your PATH."

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/sam
	rm -f $(DESTDIR)$(APPDIR)/sam.desktop

# Package the current release build into a release tarball plus checksum.
#
# The binary links only libc, libm and libgcc_s — winit and glutin dlopen the
# X11, Wayland and GL libraries at run time — so glibc is the only portability
# constraint. Whatever glibc this build was made against becomes the minimum
# for anyone using the tarball. Use `dist-portable` for something shippable.
dist: build
	rm -rf $(DIST_DIR)
	mkdir -p $(DIST_DIR)
	install -m755 target/release/sam $(DIST_DIR)/sam
	install -m644 packaging/sam.desktop $(DIST_DIR)/sam.desktop
	install -m755 packaging/install.sh $(DIST_DIR)/install.sh
	install -m644 README.md $(DIST_DIR)/README.md
	install -m644 ../LICENSE.txt $(DIST_DIR)/LICENSE.txt
	cd dist && tar czf $(DIST_NAME).tar.gz $(DIST_NAME)
	cd dist && sha256sum $(DIST_NAME).tar.gz > $(DIST_NAME).tar.gz.sha256
	@echo
	@echo "Built dist/$(DIST_NAME).tar.gz"
	@echo "Minimum glibc for this tarball:"
	@objdump -T target/release/sam | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' \
		| sort -uV | tail -1 | sed 's/^/  /'

# Build the tarball inside an old-glibc container so it runs on mainstream
# distributions rather than only on this machine. Mirrors what CI does.
# Requires a running Docker daemon.
dist-portable:
	docker run --rm -v "$(CURDIR)/..":/src -w /src/rust ubuntu:22.04 bash -c '\
		set -eux; \
		apt-get update -qq; \
		DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
			curl ca-certificates build-essential >/dev/null; \
		curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable; \
		. "$$HOME/.cargo/env"; \
		make dist'

clean:
	$(CARGO) clean
	rm -rf dist
