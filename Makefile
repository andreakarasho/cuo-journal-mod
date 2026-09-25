CFG ?= Release
# LTO on by default (the published wasm is the only artifact that ships); pass
# LTO=false for a quick single-link build while iterating.
LTO ?= true

OUT := dist/journal

.PHONY: build check test clean

# Builds the mod and assembles a drop-in folder in dist/. Copy dist/journal into
# a client's ecs-mods/ (next to the exe) to install it.
build:
	dotnet publish ecs-journal.csproj -c $(CFG) -p:OptimizationPreference=Speed -p:IlcPgoOptimize=false -p:WasmLto=$(LTO)
	mkdir -p $(OUT)
	cp bin/$(CFG)/net10.0/wasi-wasm/native/ecs_journal.wasm $(OUT)/mod.wasm
	# No "replaces" key: the claim on the client's system log is a LIVE component
	# on the window root (cuo:ui/supersedes), so it comes and goes with the window
	# instead of being a manifest fact the host has to track and undo.
	printf '{\n  "name": "journal",\n  "version": "0.3.0",\n  "wasm": "mod.wasm",\n  "ruleset": {}\n}\n' > $(OUT)/mod.json
	@echo ">> $(OUT)/mod.wasm"

check:
	dotnet build ecs-journal.csproj -c $(CFG)

clean:
	rm -rf bin obj dist tests/bin tests/obj

test:
	dotnet test tests/journal-tests.csproj
