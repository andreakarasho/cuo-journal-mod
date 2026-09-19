CFG ?= Release
# LTO on by default (the published wasm is the only artifact that ships); pass
# LTO=false for a quick single-link build while iterating.
LTO ?= true

OUT := dist/journal

.PHONY: build check clean

# Builds the mod and assembles a drop-in folder in dist/. Copy dist/journal into
# a client's ecs-mods/ (next to the exe) to install it.
build:
	dotnet publish ecs-journal.csproj -c $(CFG) -p:OptimizationPreference=Speed -p:IlcPgoOptimize=false -p:WasmLto=$(LTO)
	mkdir -p $(OUT)
	cp bin/$(CFG)/net10.0/wasi-wasm/native/ecs_journal.wasm $(OUT)/mod.wasm
	# replaces: the client takes its own system-log window down while this mod is
	# installed and enabled, so the two never stack (host side:
	# SystemLogGumpPlugin.ReplaceFeature). Top level, not in ruleset — ruleset is
	# what the host permits the mod, this is what the mod claims about itself.
	printf '{\n  "name": "journal",\n  "version": "0.2.1",\n  "wasm": "mod.wasm",\n  "replaces": ["cuo:ui/system-log"],\n  "ruleset": {}\n}\n' > $(OUT)/mod.json
	@echo ">> $(OUT)/mod.wasm"

check:
	dotnet build ecs-journal.csproj -c $(CFG)

clean:
	rm -rf bin obj dist
