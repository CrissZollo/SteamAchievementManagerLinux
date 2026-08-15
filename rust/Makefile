PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
APPDIR := $(PREFIX)/share/applications

CARGO ?= cargo

.PHONY: all build test check probe install uninstall clean

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

clean:
	$(CARGO) clean
