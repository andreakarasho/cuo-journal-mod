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
* **Resize** — drag the bottom-right corner while unlocked.
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

## Turn the built-in log off

The client draws its own bottom-left log; with the mod loaded you'd see both.
Switch it off in **Options → Interface → System Log → "Built-in message log"**.

## How it works

No bespoke host hooks — everything is existing mod surface:

| need | surface |
|---|---|
| log lines | `cuo:chat/message` trigger, `Kind == 1` (`Kind 0` is overhead speech) |
| window + tabs | `cuo:ui/node`, `bg-color`, `text`, `text-font`, `text-color`, `text-wrap`, `interaction`, `clicked` |
| drag | `cuo:ui/movable` (+ `cuo:ui/no-right-click-close`, so a stray right-click can't close it) |
| resize | `cuo:ui/resizable` — the host owns the gesture; a mod must never do rect math off the raw mouse |
| hover / fade | `cuo:input/mouse` + `cuo:engine/time` |
| line colour | the `hue_color` host import — the server's UO hue resolved to RGB, the same tint the host would paint the glyph with |
| persistence | per-mod storage (`Data/Mods/journal/storage.json`) |
