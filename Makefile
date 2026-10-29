PLUGINS := target/plugins

# Command-check examples run from temporary directories, where goenv cannot
# discover plugins/lang-go/.go-version. Keep this toolchain on PATH for checks.
GO_TOOLCHAIN := $(shell cd plugins/lang-go && go env GOROOT 2>/dev/null)
export PATH := $(GO_TOOLCHAIN)/bin:$(PATH)

.PHONY: plugins test lint ci

# Builds the bundled out-of-process plugins; each directory holds the binary
# and its lighthouse-plugin.toml, the layout plugin discovery expects. Binaries
# are replaced by rename, never overwritten in place: macOS caches a binary's
# code signature per file, and new bytes under the old file are killed on launch.
plugins:
	mkdir -p $(PLUGINS)/lang-go
	cd plugins/lang-go && go build -o ../../$(PLUGINS)/lang-go/lang-go.new ./cmd/lang-go
	mv -f $(PLUGINS)/lang-go/lang-go.new $(PLUGINS)/lang-go/lang-go
	cp plugins/lang-go/lighthouse-plugin.toml $(PLUGINS)/lang-go/
	for plugin in lang-rust lang-yaml lang-toml lang-markdown model-http model-local; do \
		mkdir -p $(PLUGINS)/$$plugin; \
		cargo build --release -p $$plugin; \
		cp target/release/$$plugin $(PLUGINS)/$$plugin/$$plugin.new; \
		mv -f $(PLUGINS)/$$plugin/$$plugin.new $(PLUGINS)/$$plugin/$$plugin; \
		cp plugins/$$plugin/lighthouse-plugin.toml $(PLUGINS)/$$plugin/; \
	done

test: plugins
	cargo test --workspace
	cd plugins/lang-go && go test ./...

# Formatters, compilers' linters, then the dogfood gate: Lighthouse checks its
# own sources with the rules this repository enforces (lighthouse.toml).
lint: plugins
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cd plugins/lang-go && test -z "$$(gofmt -l .)" && go vet ./...
	LIGHTHOUSE_TRUST=1 cargo run -q -p lighthouse-cli -- check decisions/
	LIGHTHOUSE_TRUST=1 cargo run -q -p lighthouse-cli -- check . --allow-incomplete

# What CI runs.
ci: test lint
