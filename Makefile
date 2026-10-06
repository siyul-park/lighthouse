PLUGINS := target/plugins

.PHONY: plugins test

# Builds the bundled out-of-process plugins; each directory holds the binary
# and its lighthouse-plugin.toml, the layout plugin discovery expects.
plugins:
	mkdir -p $(PLUGINS)/lang-go
	cd plugins/lang-go && go build -o ../../$(PLUGINS)/lang-go/lang-go ./cmd/lang-go
	cp plugins/lang-go/lighthouse-plugin.toml $(PLUGINS)/lang-go/
	mkdir -p $(PLUGINS)/lang-rust
	cargo build --release -p lang-rust
	cp target/release/lang-rust $(PLUGINS)/lang-rust/
	cp plugins/lang-rust/lighthouse-plugin.toml $(PLUGINS)/lang-rust/

# The last step is the dogfood gate: Lighthouse checks its own sources with
# the rules this repository enforces (lighthouse.toml) and must find nothing.
test: plugins
	cargo test --workspace
	cargo run -q -p lighthouse-cli -- check .
