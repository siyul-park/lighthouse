PLUGINS := target/plugins

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
	mkdir -p $(PLUGINS)/lang-rust
	cargo build --release -p lang-rust
	cp target/release/lang-rust $(PLUGINS)/lang-rust/lang-rust.new
	mv -f $(PLUGINS)/lang-rust/lang-rust.new $(PLUGINS)/lang-rust/lang-rust
	cp plugins/lang-rust/lighthouse-plugin.toml $(PLUGINS)/lang-rust/

test: plugins
	cargo test --workspace
	cd plugins/lang-go && go test ./...

# Formatters, compilers' linters, then the dogfood gate: Lighthouse checks its
# own sources with the rules this repository enforces (lighthouse.toml).
lint: plugins
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cd plugins/lang-go && test -z "$$(gofmt -l .)" && go vet ./...
	cargo run -q -p lighthouse-cli -- check .

# What CI runs.
ci: test lint
