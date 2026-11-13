# Two tiers.
#   make fast  the agent's gate after every step: what changed against BASE
#              (default main) and what depends on it (scripts/fast.sh).
#   make ci    the full gate, run before a merge: every test, every lint, every
#              decision example, the generated docs.
# Both build the plugins once (a stamp file) and the release lighthouse binary
# once; every check runs that binary.

PLUGINS := target/plugins
LIGHTHOUSE := target/release/lighthouse
BUILT := $(PLUGINS)/.built
SOURCES := $(shell find crates plugins protocol Cargo.toml Cargo.lock -type f ! -path '*/node_modules/*' ! -path '*/dist/*' ! -path '*/__pycache__/*' ! -path '*/target/*')

# Command-check examples run from temporary directories, where goenv cannot
# discover plugins/lang-go/.go-version. Keep this toolchain on PATH for checks.
GO_TOOLCHAIN := $(shell cd plugins/lang-go && go env GOROOT 2>/dev/null)
export PATH := $(GO_TOOLCHAIN)/bin:$(PATH)

# Test builds of the gates keep no incremental state: it grows the debug
# target by tens of gigabytes. Plain `cargo` outside make keeps its default.
export CARGO_INCREMENTAL := 0

.PHONY: plugins cli test lint decisions docs ci fast

# `decision test` of the decisions $(1) (all when empty), which fails when it
# fails and also when it skipped an example: the gate lists every plugin and
# trusts the project, so a skip is an example that never ran here. The command
# itself exits 0 on a skip, as `cargo test` does on an ignored test; the
# summary line is what says `0 skipped`.
define test-examples
out=$$(LIGHTHOUSE_TRUST=1 $(LIGHTHOUSE) decision test $(1)) || { printf '%s\n' "$$out"; exit 1; }; \
printf '%s\n' "$$out"; \
printf '%s\n' "$$out" | grep -q ', 0 skipped,' || { echo 'the gate runs every example: some were skipped' >&2; exit 1; }
endef

# Builds the out-of-process plugins; each directory holds the binary, its
# lighthouse-plugin.toml and, for a plugin that provides packs, its `decisions/`:
# the layout plugin discovery expects. Binaries are replaced by rename, never overwritten in place: macOS caches a binary's
# code signature per file, and new bytes under the old file are killed on launch.
# `-trimpath -buildvcs=false` makes the binary the same in every checkout and
# commit of the same sources: trust hashes it, so a worktree of a trusted
# repository builds the plugin its trust was given for.
plugins: $(BUILT)

$(BUILT): $(SOURCES) Makefile
	mkdir -p $(PLUGINS)/lang-go
	cd plugins/lang-go && go build -trimpath -buildvcs=false -o ../../$(PLUGINS)/lang-go/lang-go.new ./cmd/lang-go
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
	touch $(BUILT)

# The release binary every check below runs; cargo knows when it is fresh.
cli:
	cargo build --release -p lighthouse-cli

test: plugins
	cargo test --workspace
	cd plugins/lang-go && go test ./...
	cd plugins/lang-typescript && npm test
	cd plugins/lang-python && python3 -I -m unittest discover -s tests -t .

# Formatters, compilers' linters, then the dogfood gate: Lighthouse checks its
# own sources with the rules this repository enforces (lighthouse.toml).
lint: plugins cli
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cd plugins/lang-go && test -z "$$(gofmt -l .)" && go vet ./... && cd sdk && go run ./gen -check
	cd plugins/lang-python && ruff format --check . && ruff check . && python3 -I scripts/wire.py --check
	LIGHTHOUSE_TRUST=1 $(LIGHTHOUSE) check $(wildcard plugins/*/decisions)
	LIGHTHOUSE_TRUST=1 $(LIGHTHOUSE) check . --allow-incomplete
	$(call test-examples,'markdown/*')

# Every example of every checked decision, and the queries of the translated
# decisions held to the decisions on the subjects of the whole repository.
decisions: plugins cli
	$(call test-examples,)
	cargo test --release -p lighthouse-checks --test it -- --ignored the_query_holds_of_the_subjects_the_decision_reports_on_this_repository
	cargo test --release -p lighthouse-engine --lib -- --ignored every_rendered
	cargo test --release -p lighthouse-engine --test it -- --ignored witness_catalog::types_that_hold

docs: cli
	LIGHTHOUSE_TRUST=1 $(LIGHTHOUSE) docs check

# The full gate.
ci: test lint decisions docs

# The agent's gate; BASE is the ref the change is measured against.
fast:
	BASE=$(or $(BASE),main) sh scripts/fast.sh
