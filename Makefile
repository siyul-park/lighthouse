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
	mkdir -p $(PLUGINS)/lang-rust
	cargo build --release -p lang-rust
	cp target/release/lang-rust $(PLUGINS)/lang-rust/lang-rust.new
	mv -f $(PLUGINS)/lang-rust/lang-rust.new $(PLUGINS)/lang-rust/lang-rust
	cp plugins/lang-rust/lighthouse-plugin.toml $(PLUGINS)/lang-rust/
	for lang in yaml toml; do \
		mkdir -p $(PLUGINS)/lang-$$lang; \
		cargo build --release -p lang-$$lang; \
		cp target/release/lang-$$lang $(PLUGINS)/lang-$$lang/lang-$$lang.new; \
		mv -f $(PLUGINS)/lang-$$lang/lang-$$lang.new $(PLUGINS)/lang-$$lang/lang-$$lang; \
		cp plugins/lang-$$lang/lighthouse-plugin.toml $(PLUGINS)/lang-$$lang/; \
	done
	mkdir -p $(PLUGINS)/model-http
	cargo build --release -p model-http
	cp target/release/model-http $(PLUGINS)/model-http/model-http.new
	mv -f $(PLUGINS)/model-http/model-http.new $(PLUGINS)/model-http/model-http
	cp plugins/model-http/lighthouse-plugin.toml $(PLUGINS)/model-http/
	mkdir -p $(PLUGINS)/model-local
	cargo build --release -p model-local
	cp target/release/model-local $(PLUGINS)/model-local/model-local.new
	mv -f $(PLUGINS)/model-local/model-local.new $(PLUGINS)/model-local/model-local
	cp plugins/model-local/lighthouse-plugin.toml $(PLUGINS)/model-local/

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
