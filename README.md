# cuo-journal-mod

The ClassicUO 2.0 system-log window, as a mod: a tabbed, resizable, lockable
message log for the bottom-left of the screen.

* **Tabs** — All / Sys / Chat / Party / Guild, split on the message type the
  server sent. All is everything. Takes the system channel and overhead speech,
  like the client's own journal.
* **Custom tabs and rules** — the `OPT` button opens an options window: add tabs
  (a name + a filter) and rules (a filter + a new hue and/or hide). A filter is
  message type, text-contains (case-insensitive) and hue, all optional.
* **Scrollback** — mouse wheel scrolls back through the history.
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
* Position, size, lock state, the active tab, custom tabs and rules persist in
  the mod's storage.

## Build

Rust (`cargo` + the `wasm32-wasip1` target). The mod builds against the SDK
submodule (`external/classicuo-mods-sdk`); keep it on the same SDK commit as the
client you install into.

```bash
git clone --recursive https://github.com/andreakarasho/cuo-journal-mod
cd cuo-journal-mod
rustup target add wasm32-wasip1
make build                # LTO release -> ../cuo-agents/ecs-mods/journal/{mod.wasm,mod.json}
make test                 # filter/format unit tests (src/filters.rs, host target)
```

`make build CUO_REPO=/path/to/client` installs into another checkout (its
`ecs-mods/journal`); `make build OUT=dir` writes anywhere.

Release builds are stripped and LTO'd (`[profile.release]` in `Cargo.toml`).
Set `strip = false` there when you need a wasm trap to symbolicate.

Storage format is unchanged from the old C# build, so an existing
`Data/Mods/journal/storage.json` carries over.

## Publishing

CI (`.github/workflows/build.yml`) builds every push and PR. Pushing a `v*` tag
also uploads `journal.zip` (the `mod.json` + `mod.wasm` pair) to a GitHub
Release and prints its sha256 plus the ready-made registry entry for
[ClassicUO/classicuo-mods](https://github.com/ClassicUO/classicuo-mods) —
open a PR there with `mods/journal.json` to list the mod.

## It replaces the built-in log

The client draws its own bottom-left log. The journal's root window carries

```rust
types::ModSupersedes { feature: "cuo:ui/system-log".into() }
```

so the client hides its own log for as long as that window exists — no options
trip, and the two never stack. It's a live claim, nothing in `mod.json`: disable
or uninstall the mod, the host despawns its entities and the built-in log comes
straight back.

**Options → Interface → System Log → "Built-in message log"** still exists; it
turns the built-in log off when no mod is superseding it.

## How it works

No bespoke host hooks — everything is existing mod surface:

| need | surface |
|---|---|
| log lines | `cuo:chat/message` trigger, `Kind 1` (system channel) and `Kind 0` (overhead speech) |
| replace built-in log | `cuo:ui/supersedes` on the root window |
| window + tabs | `cuo:ui/node`, `bg-color`, `text`, `text-font`, `text-color`, `text-wrap`, `interaction`, parented with `cuo:ecs/child-of`; clicks are the `cuo:ui/click` event |
| drag | `cuo:ui/movable` (+ `cuo:ui/no-right-click-close`, so a stray right-click can't close it) |
| resize | `cuo:ui/resizable` — the host owns the gesture; a mod must never do rect math off the raw mouse |
| hover / fade | `cuo:input/mouse` + `cuo:engine/time` |
| line font | `cuo:ui/text-font` — `FontId` is the server's font index, `\| 0x80` for the UO ASCII set (the host's `AsciiFlag`); `Size` is ignored, UO bitmap fonts are fixed-size |
| line colour | unicode lines: the `hue_color` host import — the server's UO hue resolved to RGB, the same tint the host would paint the glyph with. ASCII lines: the raw hue packed into the colour's R/G bytes, because the host bakes the hue into those glyphs instead of tinting them |
| persistence | per-mod storage (`Data/Mods/journal/storage.json`) |
