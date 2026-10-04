OUT := dist/journal
WASM := target/wasm32-wasip1/release/cuo_journal.wasm

.PHONY: build check test clean

# Builds the mod and assembles a drop-in folder in dist/. Copy dist/journal into
# a client's ecs-mods/ (next to the exe) to install it.
build:
	cargo build --release --target wasm32-wasip1
	mkdir -p $(OUT)
	cp $(WASM) $(OUT)/mod.wasm
	# No "replaces" key: the claim on the client's system log is a LIVE component
	# on the window root (cuo:ui/supersedes), so it comes and goes with the window
	# instead of being a manifest fact the host has to track and undo.
	printf '{\n  "name": "journal",\n  "version": "0.3.0",\n  "wasm": "mod.wasm",\n  "ruleset": {}\n}\n' > $(OUT)/mod.json
	@echo ">> $(OUT)/mod.wasm"

check:
	cargo check --target wasm32-wasip1

clean:
	cargo clean
	rm -rf dist

test:
	cargo test
