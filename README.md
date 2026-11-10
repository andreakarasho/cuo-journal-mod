# cuo-journal-mod

The ClassicUO 2.0 system-log window, as a mod: a tabbed, resizable, lockable
message log for the bottom-left of the screen.

* **Tabs** — All / Sys / Chat / Party / Guild, split on the message type the
  server sent. All is everything. Takes the system channel and overhead speech,
  like the client's own journal.
* **Custom tabs and rules** — the `OPT` button opens an options window: add tabs
  (a name + a filter) and rules (a filter + a new hue and/or hide). A filter is
  message type, text-contains (case-insensitive) and hue, all optional.
* **Themed** — the windows are HTML/CSS pages (`<body data-theme>`), so the
  client's UI theme draws them. Their files are exported once to
  `Data/Mods/journal/ui/` (`journal.html` / `.css`, `options.html` / `.css`),
  where your edited copy wins and reloads on save.
* **Scrollback** — mouse wheel scrolls back through the history.
* **Idle it is invisible** — the panel goes and only the text is left, and
  locked the window is no hit target, so clicks pass through to the world.
* **Hover** — the panel fades in (~170ms) with the chrome, and the full retained
  history shows, instead of only the lines still inside their 10s.
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

Rust (`cargo` + the `wasm32-wasip2` target). The mod builds against the SDK
submodule (`external/classicuo-mods-sdk`); keep it on the same SDK commit as the
client you install into.

```bash
git clone --recursive https://github.com/andreakarasho/cuo-journal-mod
cd cuo-journal-mod
rustup target add wasm32-wasip2
make build                # LTO release -> dist/journal/{mod.wasm,mod.json}
make test                 # filter/format unit tests (src/filters.rs, host target)
```

Copy `dist/journal` into the client's `Data/Mods/`; `make build OUT=dir` writes anywhere.

Release builds are stripped and LTO'd (`[profile.release]` in `Cargo.toml`).
Set `strip = false` there when you need a wasm trap to symbolicate.

`Data/Mods/journal/storage.json` is one JSON object: window `x`/`y`/`w`/`h`,
`locked`, selected `tab` (0 = All), custom `tabs` and `rules`. A file that
doesn't parse is ignored and the window starts from its defaults.

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
| window + tabs + options | the `host.page-*` calls: `src/journal.html` / `.css` and `src/options.html` / `.css`, filled by element id; clicks are `page-clicks`, option fields `page-value` |
| drag | `cuo:ui/movable` (+ `cuo:ui/no-right-click-close`, so a stray right-click can't close it) |
| resize | `cuo:ui/resizable` — the host owns the gesture; a mod must never do rect math off the raw mouse |
| hover / fade | `cuo:input/mouse` + `cuo:engine/time` |
| line font | inline `font-family`: `uo-N` is the server's unicode font N, `uo-ascii-N` the UO ASCII set; UO bitmap fonts are fixed-size |
| line colour | inline `color: hue(N)`, the raw server hue (or a rule's): the host paints it as its own log does |
| persistence | per-mod storage (`Data/Mods/journal/storage.json`) |
