// cuo-journal-mod — the ClassicUO 2.0 system-log window, as a mod.
//
// A tabbed, resizable, lockable message log that replaces the client's built-in
// bottom-left scroll (turn that off in Options -> Interface -> System Log ->
// "Built-in message log", or the two will overlap).
//
// Everything it needs is host surface, no bespoke hooks:
//   * lines arrive as cuo:chat/message triggers with Kind == 1 (the system log
//     channel; Kind 0 is overhead speech and is ignored here);
//   * the window is plain cuo:ui/* nodes — no custom rendering;
//   * dragging is cuo:ui/movable, resizing is cuo:ui/resizable (the host owns
//     both gestures — a mod must never do rect math off the raw mouse), and
//   * cuo:ui/no-right-click-close keeps a stray right-click from closing it.
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
    const int LineSlots = 40;         // pre-spawned text nodes (the visible window)
    const int MaxLines = 60;          // retained history
    const float Lifetime = 10f;       // seconds a line shows while the window is idle
    const float FadeSpeed = 6f;       // per second
    const int IdleZ = 1;              // below every gump, like the built-in log
    const int HoverZ = 30000;         // above them while the cursor is on it

    // ---- entity names ----------------------------------------------------
    // Commands.Spawn(name) hands the real ecs id back through ctx.Entity(name)
    // on the NEXT call, so nothing is matched on a string at runtime.
    const string Root = "journal.root";
    const string Strip = "journal.strip";
    const string TabsBox = "journal.tabs";
    const string Area = "journal.area";
    const string LockBtn = "journal.lock";

    static string TabName(int i) => $"journal.tab{i}";
    static string LineName(int i) => $"journal.line{i}";

    static readonly string[] TabLabels = { "All", "Sys", "Chat", "Party", "Guild" };

    sealed class Line
    {
        public string Text = "";
        public int Tab;
        public float Expire;
        public ushort Hue;
        public Color Color = Color.Rgba(205, 210, 224, 255);
        public bool Resolved;
    }

    readonly List<Line> _lines = new();

    bool _spawned;
    bool _loaded;
    bool _dirty = true;          // lines/tab changed — repaint the text column
    bool _lastShowAll;

    float _x = 6f, _y = 320f, _w = 320f, _h = 150f;
    bool _locked = true;
    int _tab;
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
            Append(t.Event.Text, TabOf(t.Event.MessageType), t.Event.Hue);
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

    void Append(string text, int tab, ushort hue)
    {
        // Collapse an immediate repeat, like the built-in log.
        if (_lines.Count > 0 && _lines[^1].Text == text)
        {
            _lines[^1].Expire = _now + Lifetime;
            _dirty = true;
            return;
        }
        if (_lines.Count >= MaxLines)
            _lines.RemoveAt(0);
        _lines.Add(new Line { Text = text, Tab = tab, Hue = hue, Expire = _now + Lifetime });
        _dirty = true;
    }

    float _now;

    // ---- frame -----------------------------------------------------------

    void Tick(Commands cmds, ModContext ctx)
    {
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

        var time = ctx.Resource<Time>();
        var dt = time?.Frame ?? 0.016f;
        _now = time?.Total ?? (_now + dt);

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

        var target = hovered ? 1f : 0f;
        var step = dt * FadeSpeed;
        _fade = MathF.Abs(target - _fade) <= step ? target : _fade + MathF.CopySign(step, target - _fade);

        // Drag/resize end: the host writes the box, so persist once the button
        // comes back up (Save is a no-op when the blob is unchanged).
        if (mouse is { Left: false })
            Save(ctx);

        Paint(cmds, ctx);

        var showAll = _fade >= 1f;
        if (_dirty || showAll != _lastShowAll)
        {
            _lastShowAll = showAll;
            _dirty = false;
            PaintLines(cmds, ctx, showAll);
        }
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
    }

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
    }

    byte Alpha(int full) => (byte)Math.Clamp(full * _fade, 0f, 255f);

    void PaintLines(Commands cmds, ModContext ctx, bool showAll)
    {
        // Newest last: walk the tail of the list that passes the tab + freshness
        // filter, oldest first, and fill the slots bottom-up.
        var picked = new List<Line>(LineSlots);
        for (var i = _lines.Count - 1; i >= 0 && picked.Count < LineSlots; i--)
        {
            var line = _lines[i];
            if (_tab != 0 && line.Tab != _tab) continue;
            if (!showAll && line.Expire <= _now) continue;
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
            if (!line.Resolved)
            {
                // The host resolves the UO hue for us (cuo hue_color import) — the
                // same tint it would put on the glyph itself. Cached per line: the
                // import is a guest round-trip, not a table lookup.
                line.Color = line.Hue != 0 ? ctx.Ui.HueColor(line.Hue) : TabColor(line.Tab);
                line.Resolved = true;
            }
            cmds.Insert(ent, new Text { Value = line.Text });
            cmds.Insert(ent, new TextColor { Value = line.Color });
        }
    }

    // Fallback for an unhued (hue 0) line: colour it by channel.
    static Color TabColor(int tab) => tab switch
    {
        3 => Color.Rgba(120, 190, 255, 255), // party
        4 => Color.Rgba(140, 230, 150, 255), // guild / alliance
        2 => Color.Rgba(245, 245, 245, 255), // chat
        _ => Color.Rgba(205, 210, 224, 255), // system
    };

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
            Grip = 10f,
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
