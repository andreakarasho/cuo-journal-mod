# cuo-journal-mod

The ClassicUO 2.0 system-log window, as a mod: a tabbed, resizable, lockable
message log for the bottom-left of the screen.

* **Tabs** — All / Sys / Chat / Party / Guild, split on the message type the
  server sent. All is everything.
* **Idle it is invisible** — the panel fades out and only the text is left, and
  the window is never a hit target, so clicks pass through to the world.
* **Hover** — panel + chrome fade in (~170ms) and the full retained history
  shows, instead of only the lines still inside their 10s.
* **Lock** — the `LOCK` / `MOVE` button. Locked (the default) the window can't
  be dragged or resized; only the tabs stay clickable.
* **Resize** — drag the grip in the bottom-right corner while unlocked (it
  shows with the rest of the chrome, and only when unlocked).
* **UO fonts** — every line renders in the font set the server sent it for,
  ASCII or unicode, like the built-in log.
* **Repeats collapse** — a line identical to the one before it becomes
  `text [2]`, `text [3]`… instead of scrolling the window away.
* Position, size, lock state and the active tab persist in the mod's storage.

## Build

Needs the .NET 10 SDK and the submodule:

```bash
git clone --recurse-submodules https://github.com/andreakarasho/cuo-journal-mod
cd cuo-journal-mod
make build                # LTO publish -> dist/journal/{mod.wasm,mod.json}
make build LTO=false      # faster single-link build while iterating
```

Install by copying `dist/journal` into the client's `ecs-mods/` folder (next to
the executable).

Release builds are stripped (no debug info, no stack-trace metadata,
`--strip-all` on the wasm). Pass `-p:StripSymbols=false` when you need a wasm
trap to symbolicate.

## Publishing

CI (`.github/workflows/build.yml`) builds every push and PR. Pushing a `v*` tag
also uploads `journal.zip` (the `mod.json` + `mod.wasm` pair) to a GitHub
Release and prints its sha256 plus the ready-made registry entry for
[ClassicUO/classicuo-mods](https://github.com/ClassicUO/classicuo-mods) —
open a PR there with `mods/journal.json` to list the mod.

## It replaces the built-in log

The client draws its own bottom-left log. `mod.json` declares

```json
"ruleset": { "replaces": ["cuo:ui/system-log"] }
```

so the client takes that window down for as long as this mod is installed and
enabled — no options trip, and the two never stack. Disable or uninstall the
mod and the built-in log comes straight back.

**Options → Interface → System Log → "Built-in message log"** still exists; it
turns the built-in log off when no mod is replacing it.

## How it works

No bespoke host hooks — everything is existing mod surface:

| need | surface |
|---|---|
| log lines | `cuo:chat/message` trigger, `Kind == 1` (`Kind 0` is overhead speech) |
| window + tabs | `cuo:ui/node`, `bg-color`, `text`, `text-font`, `text-color`, `text-wrap`, `interaction`, `clicked` |
| drag | `cuo:ui/movable` (+ `cuo:ui/no-right-click-close`, so a stray right-click can't close it) |
| resize | `cuo:ui/resizable` — the host owns the gesture; a mod must never do rect math off the raw mouse |
| hover / fade | `cuo:input/mouse` + `cuo:engine/time` |
| line font | `cuo:ui/text-font` — `FontId` is the server's font index, `\| 0x80` for the UO ASCII set (the host's `AsciiFlag`); `Size` is ignored, UO bitmap fonts are fixed-size |
| line colour | unicode lines: the `hue_color` host import — the server's UO hue resolved to RGB, the same tint the host would paint the glyph with. ASCII lines: the raw hue packed into the colour's R/G bytes, because the host bakes the hue into those glyphs instead of tinting them |
| persistence | per-mod storage (`Data/Mods/journal/storage.json`) |
