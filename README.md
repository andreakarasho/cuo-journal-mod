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
| persistence | per-mod storage (`Data/Mods/journal/storage.json`) |

### Known gap

A mod has no UO hue → RGB resolver, so a line takes its colour from the channel
(system / chat / party / guild), not from the server's hue. The built-in log
renders the exact hue.
