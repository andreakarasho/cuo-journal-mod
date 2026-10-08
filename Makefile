# ClassicUO checkout that receives the built mod; CI overrides OUT (dist/journal).
CUO_REPO ?= ../cuo-agents
NAME := journal
OUT ?= $(CUO_REPO)/ecs-mods/$(NAME)
WASM := target/wasm32-wasip2/release/cuo_journal.wasm

.PHONY: build wasm check test clean

# Builds the mod and drops mod.wasm + mod.json into the client's ecs-mods/journal.
build: wasm
	mkdir -p $(OUT)
	cp $(WASM) $(OUT)/mod.wasm
	# No "replaces" key: the claim on the client's system log is a LIVE component
	# on the window root (cuo:ui/supersedes), so it comes and goes with the window
	# instead of being a manifest fact the host has to track and undo.
	printf '{\n  "name": "$(NAME)",\n  "version": "0.5.0",\n  "wasm": "mod.wasm",\n  "ruleset": {}\n}\n' > $(OUT)/mod.json
	@echo ">> $(OUT)/mod.wasm"

wasm:
	cargo build --release --target wasm32-wasip2

check:
	cargo check --target wasm32-wasip2

clean:
	cargo clean

test:
	cargo test
