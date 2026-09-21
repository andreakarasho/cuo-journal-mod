// cuo-journal-mod — the ClassicUO 2.0 system-log window, as a mod.
//
// A tabbed, resizable, lockable message log that replaces the client's built-in
// bottom-left scroll — the window root carries `cuo:ui/supersedes`
// ("cuo:ui/system-log"), so the client takes its own log down while this window
// exists and puts it back when it doesn't. No options trip, the two never
// stack, and disabling the mod reverts it with nothing to undo.
//
// Everything it needs is host surface, no bespoke hooks:
//   * lines arrive as cuo:chat/message triggers with Kind == 1 (the system log
//     channel; Kind 0 is overhead speech and is ignored here);
//   * the window is plain cuo:ui/* nodes — no custom rendering;
//   * lines carry cuo:ui/text-hue, so the host paints them exactly the way it
//     paints its own log: the server's hue baked in, legacy black border, and
//     the ascii/unicode font split taken from the hue the server sent;
//   * dragging is cuo:ui/movable, resizing is cuo:ui/resizable (the host owns
//     both gestures — a mod must never do rect math off the raw mouse), and
//   * cuo:ui/no-right-click-close keeps a stray right-click from closing it, and
//     cuo:ui/supersedes is how it claims the host's system-log feature.
//
// Idle the window is invisible: the panel fades out and only the text is left,
// and since mod nodes carry no UiCustom they are never a hit target, so clicks
// fall through to the world exactly like the built-in log. Hovering (or holding
// a drag, or typing into chat) fades the panel + chrome back in and shows the
// full retained history instead of just the lines still inside their 10s.

namespace CuoJournal;

public sealed class JournalMod : Mod
{
    // ---- layout ----------------------------------------------------------
    const float TabH = 16f;
    const float Pad = 4f;
    const float MinW = 140f, MinH = 60f, MaxW = 900f, MaxH = 700f;
    const ushort UiFont = 1;          // unicode UI font (RGB-tinted)
    const float LineH = 15f;
    // Bottom-right resize band. The HOST owns the gesture (UiResizable.Grip
    // below) and puts its hit square flush in the corner, so the visible grip
    // uses the same size and the same origin — a grip you can see but not grab
    // (or the reverse) is worse than none.
    const float GripSize = 10f;
    // Bit on TextFont.FontId selecting the UO ASCII font set instead of the
    // unicode one — the host's UoFontRuntime.AsciiFlag. Server text says which
    // set it was sent for, and the built-in log honours it; so does this.
    const ushort AsciiFlag = 0x80;
    const int LineSlots = 40;         // pre-spawned text nodes (the visible window)
    const int MaxLines = 60;          // retained history
    // How long a line shows while the window is idle. MILLISECONDS: cuo:engine/time
    // hands out Total as a monotonic ms clock (and Frame as seconds — they differ).
    // Same 10s the client gives its own log (TIME_DISPLAY_SYSTEM_MESSAGE_TEXT).
    const float Lifetime = 10_000f;
    const float FadeSpeed = 6f;       // per second
    const int ScrollStep = 3;         // lines per wheel notch
    const int IdleZ = 1;              // below every gump, like the built-in log
    const int HoverZ = 30000;         // above them while the cursor is on it
    // cuo:game/state — 0 Loading, 1 LoginScreen, 2 ServerSelection,
    // 3 CharacterSelection, 4 CharacterCreation, 5 LoginError, 6 GameScreen.
    const byte GameScreen = 6;

    // ---- entity names ----------------------------------------------------
    // Commands.Spawn(name) hands the real ecs id back through ctx.Entity(name)
    // on the NEXT call, so nothing is matched on a string at runtime.
    const string Root = "journal.root";
    const string Strip = "journal.strip";
    const string TabsBox = "journal.tabs";
    const string Area = "journal.area";
    const string LockBtn = "journal.lock";
    const string Grip = "journal.grip";

    static string TabName(int i) => $"journal.tab{i}";
    static string LineName(int i) => $"journal.line{i}";

    static readonly string[] TabLabels = { "All", "Sys", "Chat", "Party", "Guild" };

    sealed class Line
    {
        public string BaseText = "";  // as received, without the repeat suffix
        public string Text = "";      // what gets drawn (BaseText, or "BaseText [N]")
        public int Count = 1;
        public int Tab;
        public float Expire;
        public ushort Hue;
        public ushort FontId;         // UO font id, | AsciiFlag when the server sent ascii
    }

    readonly List<Line> _lines = new();

    bool _spawned;
    bool _loaded;
    bool _dirty = true;          // lines/tab changed — repaint the text column
    bool _lastShowAll;
    // Soonest Expire among the lines currently on an IDLE window, so the tick can
    // spot "a line just aged out" with one compare. float.MaxValue = nothing to wait
    // for (window hovered, or no fresh lines left).
    float _nextExpire = float.MaxValue;

    float _x = 6f, _y = 320f, _w = 320f, _h = 150f;
    bool _locked = true;
    int _tab;
    // Lines between the bottom of the column and the newest line in the tab —
    // 0 = pinned to the newest, which is where an idle window always sits.
    int _scroll;
    float _fade;
    bool _wasHovered;
    string _savedState = "";

    // Last values actually pushed to the host. Every write crosses the ABI, so an
    // idle window (the common case) must cost zero commands: Paint compares
    // against these and returns early when nothing moved.
    byte _paintedAlpha = 255;   // != the spawn alpha (0), so the first frame paints
    int _paintedZ = -1;
    int _paintedTab = -1;
    bool _paintedLocked;
    float _paintedW, _paintedH;

    public override void Setup(ModBuilder m)
    {
        // Host system log -> our list. Kind 1 is the system channel; Kind 0 is
        // overhead speech, which has its own place on screen.
        m.AddObserver((On<ModChatMessage> t) =>
        {
            if (t.Event.Kind != 1 || string.IsNullOrEmpty(t.Event.Text))
                return;
            Append(t.Event.Text, TabOf(t.Event.MessageType), t.Event.Hue,
                   t.Event.Font, t.Event.IsUnicode);
        });

        m.AddSystem((Commands cmds, ModContext ctx) => Tick(cmds, ctx)).InStage(Stage.Update).Label("journal-tick");

        // Clicks land as the host's one-frame cuo:ui/clicked tag; the entity id
        // alone answers which control it was.
        m.AddSystem((Query<Data, With<ModClicked>> clicked, Commands cmds, ModContext ctx) =>
        {
            if (!_spawned) return;

            for (var i = 0; i < TabLabels.Length; i++)
            {
                if (ctx.Entity(TabName(i)) is not { } tabEnt || !clicked.Contains(tabEnt)) continue;
                if (_tab != i)
                {
                    _tab = i;
                    _scroll = 0;     // a different channel, a different history
                    _dirty = true;
                    Save(ctx);
                }
                return;
            }

            if (ctx.Entity(LockBtn) is not { } lockEnt || !clicked.Contains(lockEnt)) return;
            _locked = !_locked;
            ApplyLock(cmds, ctx);
            Save(ctx);
        }).InStage(Stage.Update).Label("journal-clicks");
    }

    // ---- ingest ----------------------------------------------------------

    static int TabOf(byte messageType) => messageType switch
    {
        // MessageType: 1 System, 2 Emote, 6 Party, 9 Guild, 10 Alliance, 3 Label,
        // 4 Focus, 5 Whisper, 7 Yell, 8 Spell (host Game/Data/MessageType.cs).
        6 => 3,
        9 or 10 => 4,
        2 or 5 or 7 or 8 => 2,
        _ => 1,
    };

    void Append(string text, int tab, ushort hue, byte font, bool isUnicode)
    {
        // The server picks the font set per message; the built-in log renders
        // each line with the one it was sent for, and so does this.
        var fontId = (ushort)(isUnicode ? font : font | AsciiFlag);

        // Collapse an immediate repeat into "text [N]", like the built-in log —
        // a spammed line shouldn't scroll the window away. Hue/font follow the
        // newest copy, so a repeat that changed colour still reads correctly.
        if (_lines.Count > 0 && _lines[^1].BaseText == text)
        {
            var last = _lines[^1];
            last.Count++;
            last.Text = $"{text} [{last.Count}]";
            last.Hue = hue;
            last.FontId = fontId;
            last.Expire = _now + Lifetime;
            _dirty = true;
            return;
        }
        // Scrolled back into the history: a new arrival must not shove the view
        // down a line. Only a line this tab shows counts — the others aren't in
        // the column being scrolled. PaintLines clamps, so the MaxLines trim
        // below can't leave this pointing past the oldest line.
        if (_scroll > 0 && (_tab == 0 || tab == _tab))
            _scroll++;

        if (_lines.Count >= MaxLines)
            _lines.RemoveAt(0);
        _lines.Add(new Line
        {
            BaseText = text,
            Text = text,
            Tab = tab,
            Hue = hue,
            FontId = fontId,
            Expire = _now + Lifetime,
        });
        _dirty = true;
    }

    float _now;

    // ---- frame -----------------------------------------------------------

    void Tick(Commands cmds, ModContext ctx)
    {
        // A message log belongs to the world, not to the login and character-select
        // screens. The client gates its own the same way — SystemLogGumpPlugin runs
        // its spawn system under RunIf(GameScreen) and despawns OnExit — and a mod
        // has no RunIf, so the check lives here. Resource<T> of a struct hands back
        // default when the host has no answer, and default is Current = 0 = Loading —
        // which is the reading we want anyway: better no window than one stranded
        // over the login screen.
        if (ctx.Resource<GameStateDto>().Current != GameScreen)
        {
            if (_spawned)
                Teardown(cmds, ctx);
            return;
        }

        if (!_spawned)
        {
            LoadState(ctx);
            Spawn(cmds);
            _spawned = true;
            return;
        }

        if (ctx.Entity(Root) is not { } root)
            return;

        // The host owns the window box once it exists (drag + resize write it),
        // so read it back rather than tracking our own copy.
        if (ctx.Component<Node>(root) is { } node)
        {
            _x = node.Left.Value;
            _y = node.Top.Value;
            _w = node.Width.Value;
            _h = node.Height.Value;
        }
        else
        {
            // No Node on the root we spawned = the host deleted our subtree without
            // telling us. That is what disable→enable does (ModdingPlugin despawns
            // every entity in the mod's slot, then only re-runs ModStartup — and this
            // window is built in Update, not Startup), so without this the mod ticks
            // on forever writing components at a dead id and the window never comes
            // back until a client restart. Reset and let the next tick rebuild it.
            Forget(ctx);
            return;
        }

        var time = ctx.Resource<Time>();
        var dt = time?.Frame ?? 0.016f;          // seconds
        _now = time?.Total ?? (_now + dt * 1000f); // milliseconds

        if (!_loaded)
        {
            // Storage only answers once the host has registered the mod, which is
            // after mod_setup — so the restore lands on the first tick, not at spawn.
            _loaded = true;
            LoadState(ctx);
            ApplyLock(cmds, ctx);
            if (ctx.Entity(Root) is { } r)
                cmds.Insert(r, Box(_x, _y, _w, _h));
        }

        var mouse = ctx.Resource<MouseInputDto>();
        var inside = mouse is { } ms
            && ms.X >= _x && ms.Y >= _y && ms.X < _x + _w && ms.Y < _y + _h;
        // Holding the button after grabbing the window keeps the chrome up while
        // the host drags or resizes it, even when the cursor runs past the edge.
        var hovered = inside || (_wasHovered && mouse is { Left: true });
        _wasHovered = hovered;

        // Wheel over the window walks back through the retained history. Hovered
        // only: idle, the window is a notification strip showing the newest lines
        // and nothing else, so it drops back to the bottom when the cursor leaves.
        // cuo:input/mouse hands out the PRE-consume delta in notches (+ up), and
        // plain wheel isn't a client gesture (zoom wants ctrl), so nothing to
        // fight over — and a mod can't consume input anyway.
        if (hovered && mouse is { Wheel: var wheel } && wheel != 0f)
        {
            _scroll += (int)MathF.Round(wheel) * ScrollStep;
            _dirty = true;
        }
        else if (!hovered && _scroll != 0)
        {
            _scroll = 0;
            _dirty = true;
        }

        var target = hovered ? 1f : 0f;
        var step = dt * FadeSpeed;
        _fade = MathF.Abs(target - _fade) <= step ? target : _fade + MathF.CopySign(step, target - _fade);

        // Drag/resize end: the host writes the box, so persist once the button
        // comes back up (Save is a no-op when the blob is unchanged).
        if (mouse is { Left: false })
            Save(ctx);

        Paint(cmds, ctx);

        var showAll = _fade >= 1f;
        // A line ageing out of its lifetime changes what the idle window shows, and
        // it is the one change no event announces — without this the column keeps a
        // dead line on screen until the next message or hover happens to repaint it.
        // PaintLines leaves the next interesting moment behind so this stays a float
        // compare per frame rather than a scan.
        if (!showAll && _now >= _nextExpire)
            _dirty = true;

        if (_dirty || showAll != _lastShowAll)
        {
            _lastShowAll = showAll;
            _dirty = false;
            PaintLines(cmds, ctx, showAll);
        }
    }

    // Leaving the world: take the window down and come back to a clean slate, so
    // logging back in rebuilds it from storage exactly like a fresh login.
    // The retained lines survive — the client keeps its SystemMessages store across
    // the same transition, and a log that forgets on every logout is no log.
    void Teardown(Commands cmds, ModContext ctx)
    {
        cmds.Despawn(Root);   // takes the subtree with it, host-side
        Forget(ctx);
    }

    // Drop every binding + cached value tied to a window that no longer exists, so
    // the next tick spawns a fresh one. Split out of Teardown because the host can
    // delete the subtree on its own (disable/enable), and then there is nothing left
    // to despawn — only the guest-side bookkeeping to clear.
    void Forget(ModContext ctx)
    {
        // The subtree despawn frees the ROOT's name only. A child name left pointing
        // at a dead id would, if the host ever recycles ids, have us writing this
        // window's components onto somebody else's entity — so drop every binding.
        // Spawn() re-registers all of them. Root included — Despawn(name) drops its
        // binding, but the host-despawn path never went through Despawn at all.
        ctx.Forget(Root);
        ctx.Forget(Strip);
        ctx.Forget(TabsBox);
        ctx.Forget(Area);
        ctx.Forget(LockBtn);
        ctx.Forget(Grip);
        for (var i = 0; i < TabLabels.Length; i++)
            ctx.Forget(TabName(i));
        for (var i = 0; i < LineSlots; i++)
            ctx.Forget(LineName(i));

        _spawned = false;
        _loaded = false;            // restore the box + re-apply the lock on respawn
        _dirty = true;
        _nextExpire = float.MaxValue;
        _scroll = 0;
        _fade = 0f;
        _wasHovered = false;
        // Paint short-circuits on "nothing moved", so the cached values have to look
        // impossible or the rebuilt window would keep the old chrome for a frame.
        _paintedAlpha = 255;
        _paintedZ = -1;
        _paintedTab = -1;
        _paintedW = 0f;
        _paintedH = 0f;
    }

    // ---- ui --------------------------------------------------------------

    static Node Box(float x, float y, float w, float h) => Node.Abs(x, y, w, h);

    void Spawn(Commands cmds)
    {
        var root = cmds.Spawn(Root)
            .With(Box(_x, _y, _w, _h))
            .With(new BackgroundColor { Value = Color.Rgba(18, 20, 26, 0) })
            .With(BorderRadius.All(4))
            .With(new GlobalZIndex { Value = IdleZ })
            // Right-click must not close the log; drag/resize are added by
            // ApplyLock once the saved lock state is known.
            .With<UiNoRightClickClose>()
            // THIS window is the system log while it exists: the client hides its
            // own bottom-left scroll and brings it back the moment this entity is
            // gone (mod disabled/unloaded -> host despawns our entities). Live
            // claim, so nothing has to be declared in mod.json or undone on exit.
            .With(new ModSupersedes { Feature = "cuo:ui/system-log" })
            .With<UiContainsByBounds>()
            .With(new UiName { Value = Root });

        // Tab strip — flow row, so the tabs size to their labels and the lock
        // button rides the right edge at any width.
        var strip = cmds.Spawn(Strip)
            .With(Row(0f, 0f, _w, TabH))
            .With(new BackgroundColor { Value = Color.Rgba(38, 41, 51, 0) })
            .With(new UiName { Value = Strip })
            .ChildOf(root);

        var tabs = cmds.Spawn(TabsBox)
            .With(Grow(TabH))
            .With(new UiName { Value = TabsBox })
            .ChildOf(strip);

        for (var i = 0; i < TabLabels.Length; i++)
        {
            cmds.Spawn(TabName(i))
                .With(TabNode())
                .With(new BackgroundColor { Value = Color.Rgba(38, 41, 51, 0) })
                .With(BorderRadius.All(3))
                .With(new Text { Value = TabLabels[i] })
                .With(new TextFont { FontId = UiFont, Size = 11 })
                .With(new TextColor { Value = Color.Rgba(160, 164, 178, 0) })
                .With(Interaction.None)
                .With<UiNoWindowDrag>()
                .With(new UiName { Value = TabName(i) })
                .ChildOf(tabs);
        }

        cmds.Spawn(LockBtn)
            .With(TabNode())
            .With(new BackgroundColor { Value = Color.Rgba(58, 48, 48, 0) })
            .With(BorderRadius.All(3))
            .With(new Text { Value = "LOCK" })
            .With(new TextFont { FontId = UiFont, Size = 11 })
            .With(new TextColor { Value = Color.Rgba(210, 160, 160, 0) })
            .With(Interaction.None)
            .With<UiNoWindowDrag>()
            .With(new UiName { Value = LockBtn })
            .ChildOf(strip);

        // Text column: bottom-aligned, clipped, so lines stack upward from the
        // bottom edge like the built-in log.
        var area = cmds.Spawn(Area)
            .With(Column(Pad, TabH, _w - Pad * 2f, _h - TabH - 2f))
            .With(new UiName { Value = Area })
            .ChildOf(root);

        for (var i = 0; i < LineSlots; i++)
        {
            cmds.Spawn(LineName(i))
                .With(LineNode())
                .With(new Text { Value = "" })
                .With(new TextFont { FontId = UiFont, Size = 13 })
                .With(new TextColor { Value = Color.Rgba(235, 238, 244, 255) })
                .With(new TextWrap { Kind = TextWrapKind.Words })
                .With(new UiName { Value = LineName(i) })
                .ChildOf(area);
        }

        // Resize grip. Purely a handle to look at — ApplyLock's UiResizable is
        // what actually resizes, and the host's grab band is this same corner
        // square. Chrome, and unlocked-only: a locked window can't be resized,
        // so it must not advertise a grip (Paint drives both).
        cmds.Spawn(Grip)
            .With(GripNode(_w, _h))
            .With(new BackgroundColor { Value = Color.Rgba(96, 102, 120, 0) })
            .With(BorderRadius.All(3))
            .With(new UiName { Value = Grip })
            .ChildOf(root);
    }

    // Flush to the bottom-right corner — UiResizablePlugin's hit square is
    // (Left + Width - Grip, Top + Height - Grip), so no inset here either.
    static Node GripNode(float w, float h)
        => Node.Abs(w - GripSize, h - GripSize, GripSize, GripSize);

    static Node Row(float x, float y, float w, float h)
    {
        var n = Node.Abs(x, y, w, h);
        n.FlexDirection = FlexDirection.Row;
        n.AlignItems = AlignItems.Center;
        n.Gap = Val.Px(2f);
        n.Padding = new UiRect { Left = Val.Px(2f), Right = Val.Px(2f), Top = Val.Px(0f), Bottom = Val.Px(0f) };
        return n;
    }

    static Node Grow(float h)
    {
        var n = Node.Base();
        n.FlexDirection = FlexDirection.Row;
        n.AlignItems = AlignItems.Center;
        n.Overflow = Overflow.Clip;
        n.Width = Val.Grow();
        n.Height = Val.Px(h);
        n.Gap = Val.Px(2f);
        return n;
    }

    static Node TabNode()
    {
        var n = Node.Base();
        n.Height = Val.Px(TabH - 2f);
        n.JustifyContent = JustifyContent.Center;
        n.AlignItems = AlignItems.Center;
        n.Padding = new UiRect { Left = Val.Px(5f), Right = Val.Px(5f), Top = Val.Px(0f), Bottom = Val.Px(0f) };
        return n;
    }

    static Node Column(float x, float y, float w, float h)
    {
        var n = Node.Abs(x, y, w, h);
        n.FlexDirection = FlexDirection.Column;
        n.JustifyContent = JustifyContent.End;
        n.Overflow = Overflow.Clip;
        return n;
    }

    static Node LineNode()
    {
        var n = Node.Base();
        n.Width = Val.Percent(100f);
        n.Height = Val.Auto();
        n.MinHeight = Val.Px(LineH);
        return n;
    }

    // Per-frame repaint of everything the fade touches. Colours only — the
    // geometry belongs to the host once the window is up.
    void Paint(Commands cmds, ModContext ctx)
    {
        var alpha = Alpha(255);
        if (alpha == _paintedAlpha && _tab == _paintedTab && _locked == _paintedLocked
            && _w == _paintedW && _h == _paintedH)
            return;
        var sizeChanged = _w != _paintedW || _h != _paintedH;
        _paintedAlpha = alpha;
        _paintedTab = _tab;
        _paintedLocked = _locked;
        _paintedW = _w;
        _paintedH = _h;

        if (ctx.Entity(Root) is { } root)
        {
            cmds.Insert(root, new BackgroundColor { Value = Color.Rgba(18, 20, 26, Alpha(180)) });
            // Idle the log sits UNDER every gump (like the built-in one); hovering
            // lifts it so the tabs aren't buried by whatever is on top.
            var z = alpha > 0 ? HoverZ : IdleZ;
            if (z != _paintedZ)
            {
                _paintedZ = z;
                cmds.Insert(root, new GlobalZIndex { Value = z });
            }
        }

        if (ctx.Entity(Strip) is { } strip)
        {
            cmds.Insert(strip, new BackgroundColor { Value = Color.Rgba(38, 41, 51, Alpha(235)) });
            var row = Row(0f, 0f, _w, TabH);
            row.Display = _fade > 0.01f ? Display.Flex : Display.None;
            cmds.Insert(strip, row);
        }

        for (var i = 0; i < TabLabels.Length; i++)
        {
            if (ctx.Entity(TabName(i)) is not { } tab) continue;
            var active = i == _tab;
            cmds.Insert(tab, new BackgroundColor
            {
                Value = active
                    ? Color.Rgba(74, 108, 158, Alpha(245))
                    : Color.Rgba(38, 41, 51, Alpha(235)),
            });
            cmds.Insert(tab, new TextColor
            {
                Value = active
                    ? Color.Rgba(236, 238, 244, Alpha(255))
                    : Color.Rgba(160, 164, 178, Alpha(255)),
            });
        }

        if (ctx.Entity(LockBtn) is { } lockBtn)
        {
            // Locked reads brass-ish, unlocked red — same convention the built-in
            // window uses for its padlock (the gump set has no open-padlock art).
            cmds.Insert(lockBtn, new Text { Value = _locked ? "LOCK" : "MOVE" });
            cmds.Insert(lockBtn, new TextColor
            {
                Value = _locked
                    ? Color.Rgba(226, 190, 120, Alpha(255))
                    : Color.Rgba(235, 120, 120, Alpha(255)),
            });
            cmds.Insert(lockBtn, new BackgroundColor { Value = Color.Rgba(48, 44, 52, Alpha(235)) });
        }

        // The text column tracks the window box (the host resizes the ROOT only;
        // absolute children keep their own size). Only on a real resize.
        if (sizeChanged && ctx.Entity(Area) is { } area)
            cmds.Insert(area, Column(Pad, TabH, _w - Pad * 2f, _h - TabH - 2f));

        if (ctx.Entity(Grip) is { } grip)
        {
            cmds.Insert(grip, new BackgroundColor { Value = Color.Rgba(96, 102, 120, Alpha(235)) });
            var g = GripNode(_w, _h);
            g.Display = alpha > 0 && !_locked ? Display.Flex : Display.None;
            cmds.Insert(grip, g);
        }
    }

    byte Alpha(int full) => (byte)Math.Clamp(full * _fade, 0f, 255f);

    void PaintLines(Commands cmds, ModContext ctx, bool showAll)
    {
        // Newest last: walk the tail of the list that passes the tab + freshness
        // filter, oldest first, and fill the slots bottom-up.
        //
        // Along the way, remember when the SHOWN set next changes by itself — the
        // soonest expiry among the lines actually on screen. Tick watches that one
        // value instead of re-scanning, and while hovered nothing expires out of
        // view at all, so there is nothing to watch.
        _nextExpire = float.MaxValue;

        // Scroll offset, clamped against what this tab actually holds — the wheel
        // is free to run past either end, and one line always stays on screen so
        // the window never goes blank. 60 lines, so counting beats tracking it.
        var matching = 0;
        foreach (var l in _lines)
            if (_tab == 0 || l.Tab == _tab) matching++;
        _scroll = Math.Clamp(_scroll, 0, Math.Max(0, matching - 1));
        // Idle shows the newest lines and only those: there is nothing to scroll
        // when what you can see is "whatever arrived in the last 10 seconds".
        var skip = showAll ? _scroll : 0;

        var picked = new List<Line>(LineSlots);
        for (var i = _lines.Count - 1; i >= 0 && picked.Count < LineSlots; i--)
        {
            var line = _lines[i];
            if (_tab != 0 && line.Tab != _tab) continue;
            if (skip > 0) { skip--; continue; }
            if (!showAll)
            {
                if (line.Expire <= _now) continue;
                if (line.Expire < _nextExpire) _nextExpire = line.Expire;
            }
            picked.Add(line);
        }
        picked.Reverse();

        for (var slot = 0; slot < LineSlots; slot++)
        {
            if (ctx.Entity(LineName(slot)) is not { } ent) continue;
            var used = slot >= LineSlots - picked.Count;
            var node = LineNode();
            node.Display = used ? Display.Flex : Display.None;
            cmds.Insert(ent, node);
            if (!used) continue;

            var line = picked[slot - (LineSlots - picked.Count)];
            // Hand the host the RAW server hue and let it paint the line exactly as
            // the built-in log does — baked colour, legacy black border, and the
            // ascii/unicode split resolved from FontId. Converting the hue here
            // instead would be both flatter (no border) and wrong: hue_color answers
            // GetPolygoneColor(30, hue + 1), the wire convention chat and nameplates
            // use, while text baking answers GetPolygoneColor(30, hue) — one table
            // entry apart. Letting the host do it also drops a guest round-trip per
            // line, so there is nothing left to cache.
            cmds.Insert(ent, new Text { Value = line.Text });
            cmds.Insert(ent, new TextFont { FontId = line.FontId, Size = 13 });
            cmds.Insert(ent, new TextHue { Value = line.Hue });
        }
    }

    // ---- lock / persistence ---------------------------------------------

    // Locked = no drag, no resize. Both gestures are host-owned markers, so this
    // is just add/remove.
    void ApplyLock(Commands cmds, ModContext ctx)
    {
        if (ctx.Entity(Root) is not { } root) return;
        if (_locked)
        {
            cmds.Remove<UiMovable>(root);
            cmds.Remove<UiResizable>(root);
            return;
        }
        cmds.Insert<UiMovable>(root);
        cmds.Insert(root, new UiResizable
        {
            MinW = MinW, MinH = MinH,
            MaxW = MaxW, MaxH = MaxH,
            Grip = GripSize,   // same square the visible grip draws
        });
    }

    // Geometry + lock + tab as one CSV blob: the SDK's typed storage needs a
    // source-generated JSON context, and six numbers don't earn one.
    void LoadState(ModContext ctx)
    {
        var raw = ctx.Storage.GetRaw();
        if (string.IsNullOrEmpty(raw)) return;
        var parts = raw.Split(';');
        if (parts.Length < 6) return;
        if (float.TryParse(parts[0], out var x)) _x = x;
        if (float.TryParse(parts[1], out var y)) _y = y;
        if (float.TryParse(parts[2], out var w)) _w = Math.Clamp(w, MinW, MaxW);
        if (float.TryParse(parts[3], out var h)) _h = Math.Clamp(h, MinH, MaxH);
        _locked = parts[4] == "1";
        if (int.TryParse(parts[5], out var tab)) _tab = Math.Clamp(tab, 0, TabLabels.Length - 1);
        _savedState = raw;
    }

    void Save(ModContext ctx)
    {
        var blob = $"{_x};{_y};{_w};{_h};{(_locked ? 1 : 0)};{_tab}";
        if (blob == _savedState) return;
        _savedState = blob;
        ctx.Storage.SetRaw(blob);
    }
}
