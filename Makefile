PLUGINS := target/plugins

.PHONY: plugins test

# Builds the bundled out-of-process plugins; each directory holds the binary
# and its lighthouse-plugin.toml, the layout plugin discovery expects.
plugins:
	mkdir -p $(PLUGINS)/lang-go
	cd plugins/lang-go && go build -o ../../$(PLUGINS)/lang-go/lang-go ./cmd/lang-go
	cp plugins/lang-go/lighthouse-plugin.toml $(PLUGINS)/lang-go/

test: plugins
	cargo test --workspace
