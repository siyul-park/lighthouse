PLUGINS := target/plugins

# Command-check examples run from temporary directories, where goenv cannot
# discover plugins/lang-go/.go-version. Keep this toolchain on PATH for checks.
GO_TOOLCHAIN := $(shell cd plugins/lang-go && go env GOROOT 2>/dev/null)
export PATH := $(GO_TOOLCHAIN)/bin:$(PATH)

.PHONY: plugins test lint ci

# Builds the out-of-process plugins; each directory holds the binary, its
# lighthouse-plugin.toml and, for a plugin that provides packs, its `decisions/`:
# the layout plugin discovery expects. Binaries are replaced by rename, never overwritten in place: macOS caches a binary's
# code signature per file, and new bytes under the old file are killed on launch.
plugins:
	mkdir -p $(PLUGINS)/lang-go
	cd plugins/lang-go && go build -o ../../$(PLUGINS)/lang-go/lang-go.new ./cmd/lang-go
	mv -f $(PLUGINS)/lang-go/lang-go.new $(PLUGINS)/lang-go/lang-go
	cp plugins/lang-go/lighthouse-plugin.toml $(PLUGINS)/lang-go/
	rm -rf $(PLUGINS)/lang-go/decisions && cp -R plugins/lang-go/decisions $(PLUGINS)/lang-go/
	rm -rf $(PLUGINS)/lang-typescript.new
	mkdir -p $(PLUGINS)/lang-typescript.new/dist
	cd plugins/lang-typescript && npm ci --ignore-scripts && npm run build
	cp -R plugins/lang-typescript/dist/src $(PLUGINS)/lang-typescript.new/dist/
	cp plugins/lang-typescript/lang-typescript plugins/lang-typescript/lighthouse-plugin.toml plugins/lang-typescript/package.json plugins/lang-typescript/package-lock.json $(PLUGINS)/lang-typescript.new/
	cd $(PLUGINS)/lang-typescript.new && npm ci --omit=dev --ignore-scripts
	rm -rf $(PLUGINS)/lang-typescript
	mv $(PLUGINS)/lang-typescript.new $(PLUGINS)/lang-typescript
	rm -rf $(PLUGINS)/lang-python.new
	mkdir -p $(PLUGINS)/lang-python.new
	cp -R plugins/lang-python/lang_python $(PLUGINS)/lang-python.new/
	cp plugins/lang-python/lang-python plugins/lang-python/lighthouse-plugin.toml $(PLUGINS)/lang-python.new/
	find $(PLUGINS)/lang-python.new -name __pycache__ -prune -exec rm -rf {} +
	rm -rf $(PLUGINS)/lang-python
	mv $(PLUGINS)/lang-python.new $(PLUGINS)/lang-python
	for plugin in lang-rust lang-yaml lang-toml lang-markdown model-http model-local; do \
		mkdir -p $(PLUGINS)/$$plugin; \
		cargo build --release -p $$plugin; \
		cp target/release/$$plugin $(PLUGINS)/$$plugin/$$plugin.new; \
		mv -f $(PLUGINS)/$$plugin/$$plugin.new $(PLUGINS)/$$plugin/$$plugin; \
		cp plugins/$$plugin/lighthouse-plugin.toml $(PLUGINS)/$$plugin/; \
		if [ -d plugins/$$plugin/decisions ]; then \
			rm -rf $(PLUGINS)/$$plugin/decisions; \
			cp -R plugins/$$plugin/decisions $(PLUGINS)/$$plugin/; \
		fi; \
	done

test: plugins
	cargo test --workspace
	cd plugins/lang-go && go test ./...
	cd plugins/lang-typescript && npm test
	cd plugins/lang-python && python3 -I -m unittest discover -s tests -t .

# Formatters, compilers' linters, then the dogfood gate: Lighthouse checks its
# own sources with the rules this repository enforces (lighthouse.toml).
lint: plugins
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cd plugins/lang-go && test -z "$$(gofmt -l .)" && go vet ./... && cd sdk && go run ./gen -check
	cd plugins/lang-python && ruff format --check . && ruff check . && python3 -I scripts/wire.py --check
	LIGHTHOUSE_TRUST=1 cargo run -q -p lighthouse-cli -- check $(wildcard plugins/*/decisions)
	LIGHTHOUSE_TRUST=1 cargo run -q -p lighthouse-cli -- check . --allow-incomplete
	LIGHTHOUSE_TRUST=1 cargo run -q -p lighthouse-cli -- decision test 'markdown/*'

# What CI runs.
ci: test lint
