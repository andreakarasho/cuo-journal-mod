// cuo-journal-mod — the ClassicUO 2.0 system-log window, as a mod.
//
// A tabbed, resizable, lockable message log that replaces the client's built-in
// bottom-left scroll — the window root carries `cuo:ui/supersedes`
// ("cuo:ui/system-log"), so the client takes its own log down while this window
// exists and puts it back when it doesn't. No options trip, the two never
// stack, and disabling the mod reverts it with nothing to undo.
//
// Everything it needs is host surface, no bespoke hooks:
//   * lines arrive as cuo:chat/message triggers: Kind 1 (the system log channel)
//     and Kind 0 (overhead speech), taken the way the client's own journal takes
//     them (Filters.Journalled / Format);
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
//
// The OPT button on the strip opens the options window: custom tabs (a name + a
// filter) and rules (a filter + a new hue and/or hide). A filter is message type,
// text-contains (case-insensitive) and hue, all optional — Filters.cs holds the
// matching and the storage format, SDK-free so tests/ can check it.

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
    const string OptBtn = "journal.optbtn";

    // Options window. Separate root: it outlives journal rebuilds (adding a tab
    // respawns the journal strip) and never fades.
    const string Opt = "journal.opt";
    const string OptClose = "journal.opt.close";
    const string OptAddTab = "journal.opt.addtab";
    const string OptAddRule = "journal.opt.addrule";
    // ponytail: hard caps instead of a scrolling list, the window just grows.
    const int MaxCustomTabs = 8, MaxRules = 10;
    const float OptW = 460f, OptRowH = 18f, OptCapH = 14f, OptGap = 3f, OptPad = 6f;
    const int OptZ = HoverZ + 1;
    static string TabField(int i, string f) => $"journal.opt.t{i}.{f}";
    static string RuleField(int i, string f) => $"journal.opt.r{i}.{f}";

    static string TabName(int i) => $"journal.tab{i}";
    static string LineName(int i) => $"journal.line{i}";

    static readonly string[] TabLabels = { "All", "Sys", "Chat", "Party", "Guild" };

    readonly List<CustomTab> _tabs = new();
    readonly List<Rule> _rules = new();
    int TabCount => TabLabels.Length + _tabs.Count;
    string TabLabel(int i) => i < TabLabels.Length ? TabLabels[i]
        : _tabs[i - TabLabels.Length].Name is { Length: > 0 } n ? n : "?";

    sealed class Line
    {
        public string BaseText = "";  // as received, without the repeat suffix
        public string Text = "";      // what gets drawn (BaseText, or "BaseText [N]")
        public int Count = 1;
        public int Tab;               // built-in tab (1..4)
        public byte Type;             // host MessageType, for the filters
        public float Expire;
        public ushort Hue;            // as the server sent it; rules recolour at paint
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
    bool _paintedOpt;

    bool _optOpen;
    bool _optSpawned;
    bool _optRebuild;          // add/remove: despawn now, respawn next tick
    float _paintedW, _paintedH;

    public override void Setup(ModBuilder m)
    {
        // Host chat -> our list: the system channel (Kind 1) AND overhead speech
        // (Kind 0), like the client's journal. The host routes each line to exactly
        // one of the two, so nothing lands twice.
        m.AddObserver((On<ModChatMessage> t) =>
        {
            var e = t.Event;
            if (string.IsNullOrEmpty(e.Text) || !Filters.Journalled(e.Kind, e.MessageType))
                return;
            Append(Filters.Format(e.Kind, e.MessageType, e.Name ?? "", e.Text), e.MessageType,
                   Filters.TabOf(e.Kind, e.MessageType), e.Hue, e.Font, e.IsUnicode);
        });

        m.AddSystem((Commands cmds, ModContext ctx) => Tick(cmds, ctx)).InStage(Stage.Update).Label("journal-tick");

        // Clicks land as the host's one-frame cuo:ui/clicked tag; the entity id
        // alone answers which control it was.
        m.AddSystem((Query<Data, With<ModClicked>> clicked, Commands cmds, ModContext ctx) =>
        {
            if (!_spawned) return;
            bool Hit(string name) => ctx.Entity(name) is { } e && clicked.Contains(e);

            for (var i = 0; i < TabCount; i++)
            {
                if (!Hit(TabName(i))) continue;
                if (_tab != i)
                {
                    _tab = i;
                    _scroll = 0;     // a different channel, a different history
                    _dirty = true;
                    Save(ctx);
                }
                return;
            }

            if (Hit(LockBtn))
            {
                _locked = !_locked;
                ApplyLock(cmds, ctx);
                Save(ctx);
                return;
            }

            if (Hit(OptBtn) || (_optSpawned && Hit(OptClose)))
            {
                _optOpen = !_optOpen;
                _optRebuild = true;
                return;
            }

            if (_optSpawned)
                OptClick(Hit, cmds, ctx);
        }).InStage(Stage.Update).Label("journal-clicks");

        // Typing in an options field: the host's editor writes the field's Text, so
        // a Changed<Text> query over our editable nodes is the whole edit feed.
        m.AddSystem((Query<Data<Text>, Filter<Changed<Text>, With<EditableText>, With<ModEntity>>> edited, ModContext ctx, Commands cmds) =>
        {
            if (!_optSpawned) return;
            foreach (var (e, text) in edited)
                OptEdit(e, text.Value ?? "", cmds, ctx);
        }).InStage(Stage.Update).Label("journal-opt-edit");
    }

    // ---- ingest ----------------------------------------------------------

    void Append(string text, byte type, int tab, ushort hue, byte font, bool isUnicode)
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
            last.Type = type;
            last.FontId = fontId;
            last.Expire = _now + Lifetime;
            _dirty = true;
            return;
        }
        var line = new Line
        {
            BaseText = text,
            Text = text,
            Tab = tab,
            Type = type,
            Hue = hue,
            FontId = fontId,
            Expire = _now + Lifetime,
        };
        // Scrolled back into the history: a new arrival must not shove the view
        // down a line. Only a line this tab shows counts — the others aren't in
        // the column being scrolled. PaintLines clamps, so the MaxLines trim
        // below can't leave this pointing past the oldest line.
        if (_scroll > 0 && Shown(line))
            _scroll++;

        if (_lines.Count >= MaxLines)
            _lines.RemoveAt(0);
        _lines.Add(line);
        _dirty = true;
    }

    bool Matches(Filter f, Line l) => f.Matches(l.Type, l.BaseText, l.Hue);

    // In the current tab and not hidden by a rule.
    bool Shown(Line l)
    {
        foreach (var r in _rules)
            if (r.Hide && Matches(r.Filter, l)) return false;
        if (_tab == 0) return true;
        return _tab < TabLabels.Length ? l.Tab == _tab : Matches(_tabs[_tab - TabLabels.Length].Filter, l);
    }

    // First rule with a colour that matches wins; otherwise the server's hue.
    ushort HueOf(Line l)
    {
        foreach (var r in _rules)
            if (r.Color >= 0 && Matches(r.Filter, l)) return (ushort)r.Color;
        return l.Hue;
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
            if (_optSpawned)
            {
                cmds.Despawn(Opt);
                ForgetOpt(ctx);
            }
            _optOpen = false;
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
            // The options window went down with the rest of the slot.
            Forget(ctx);
            ForgetOpt(ctx);
            _optOpen = false;
            return;
        }

        TickOpt(cmds, ctx);

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
        ctx.Forget(OptBtn);
        for (var i = 0; i < TabLabels.Length + MaxCustomTabs; i++)
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

        for (var i = 0; i < TabCount; i++)
        {
            cmds.Spawn(TabName(i))
                .With(TabNode())
                .With(new BackgroundColor { Value = Color.Rgba(38, 41, 51, 0) })
                .With(BorderRadius.All(3))
                .With(new Text { Value = TabLabel(i) })
                .With(new TextFont { FontId = UiFont, Size = 11 })
                .With(new TextColor { Value = Color.Rgba(160, 164, 178, 0) })
                .With(Interaction.None)
                .With<UiNoWindowDrag>()
                .With(new UiName { Value = TabName(i) })
                .ChildOf(tabs);
        }

        cmds.Spawn(OptBtn)
            .With(TabNode())
            .With(new BackgroundColor { Value = Color.Rgba(48, 44, 52, 0) })
            .With(BorderRadius.All(3))
            .With(new Text { Value = "OPT" })
            .With(new TextFont { FontId = UiFont, Size = 11 })
            .With(new TextColor { Value = Color.Rgba(160, 164, 178, 0) })
            .With(Interaction.None)
            .With<UiNoWindowDrag>()
            .With(new UiName { Value = OptBtn })
            .ChildOf(strip);

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
            && _optOpen == _paintedOpt && _w == _paintedW && _h == _paintedH)
            return;
        var sizeChanged = _w != _paintedW || _h != _paintedH;
        _paintedAlpha = alpha;
        _paintedTab = _tab;
        _paintedLocked = _locked;
        _paintedOpt = _optOpen;
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

        for (var i = 0; i < TabCount; i++)
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

        if (ctx.Entity(OptBtn) is { } optBtn)
        {
            cmds.Insert(optBtn, new TextColor
            {
                Value = _optOpen
                    ? Color.Rgba(236, 238, 244, Alpha(255))
                    : Color.Rgba(160, 164, 178, Alpha(255)),
            });
            cmds.Insert(optBtn, new BackgroundColor
            {
                Value = _optOpen
                    ? Color.Rgba(74, 108, 158, Alpha(245))
                    : Color.Rgba(48, 44, 52, Alpha(235)),
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
            if (Shown(l)) matching++;
        _scroll = Math.Clamp(_scroll, 0, Math.Max(0, matching - 1));
        // Idle shows the newest lines and only those: there is nothing to scroll
        // when what you can see is "whatever arrived in the last 10 seconds".
        var skip = showAll ? _scroll : 0;

        var picked = new List<Line>(LineSlots);
        for (var i = _lines.Count - 1; i >= 0 && picked.Count < LineSlots; i--)
        {
            var line = _lines[i];
            if (!Shown(line)) continue;
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
            cmds.Insert(ent, new TextHue { Value = HueOf(line) });
        }
    }

    // ---- options window --------------------------------------------------

    static readonly string[] TabFields = { "name", "type", "text", "hue", "del" };
    static readonly string[] RuleFields = { "type", "text", "hue", "color", "hide", "del" };

    void TickOpt(Commands cmds, ModContext ctx)
    {
        if (_optRebuild)
        {
            _optRebuild = false;
            if (_optSpawned)
            {
                // Despawn now, spawn next tick: a name can't be re-bound in the same
                // frame that frees it.
                cmds.Despawn(Opt);
                ForgetOpt(ctx);
                return;
            }
        }
        if (_optOpen && !_optSpawned)
        {
            SpawnOpt(cmds);
            _optSpawned = true;
        }
    }

    void ForgetOpt(ModContext ctx)
    {
        ctx.Forget(Opt);
        ctx.Forget(OptClose);
        ctx.Forget(OptAddTab);
        ctx.Forget(OptAddRule);
        for (var i = 0; i < MaxCustomTabs; i++)
            foreach (var f in TabFields) ctx.Forget(TabField(i, f));
        for (var i = 0; i < MaxRules; i++)
            foreach (var f in RuleFields) ctx.Forget(RuleField(i, f));
        _optSpawned = false;
    }

    void SpawnOpt(Commands cmds)
    {
        // Children: title, caption, tab rows, add, caption, rule rows, add.
        var rows = _tabs.Count + _rules.Count;
        var h = OptPad * 2f + OptRowH * (rows + 3) + OptCapH * 2f + OptGap * (rows + 4);
        var box = Node.Abs(_x, Math.Max(0f, _y - h - 4f), OptW, h);
        box.FlexDirection = FlexDirection.Column;
        box.Padding = UiRect.Splat(Val.Px(OptPad));
        box.Gap = Val.Px(OptGap);
        var root = cmds.Spawn(Opt)
            .With(box)
            .With(new BackgroundColor { Value = Color.Rgba(18, 20, 26, 235) })
            .With(BorderRadius.All(4))
            .With(new GlobalZIndex { Value = OptZ })
            .With<UiMovable>()
            .With<UiNoRightClickClose>()
            .With<UiContainsByBounds>()
            .With(Interaction.None)
            .With(new UiName { Value = Opt });

        var title = OptRow(cmds, root);
        OptLabel(cmds, title, "Journal options", OptW - OptPad * 2f - 28f, OptRowH, 236);
        OptButton(cmds, title, OptClose, "X", 20f);

        OptLabel(cmds, root, "Tabs:  name · type · text contains · hue", OptW, OptCapH, 150);
        for (var i = 0; i < _tabs.Count; i++)
        {
            var t = _tabs[i];
            var row = OptRow(cmds, root);
            OptField(cmds, row, TabField(i, "name"), 90f, t.Name);
            OptButton(cmds, row, TabField(i, "type"), Filters.TypeName(t.Filter.Type), 70f);
            OptField(cmds, row, TabField(i, "text"), 170f, t.Filter.Text);
            OptField(cmds, row, TabField(i, "hue"), 60f, Filters.HueText(t.Filter.Hue));
            OptButton(cmds, row, TabField(i, "del"), "x", 20f);
        }
        OptButton(cmds, OptRow(cmds, root), OptAddTab, "+ Tab", 60f);

        OptLabel(cmds, root, "Rules:  type · text contains · hue · new hue · hide", OptW, OptCapH, 150);
        for (var i = 0; i < _rules.Count; i++)
        {
            var r = _rules[i];
            var row = OptRow(cmds, root);
            OptButton(cmds, row, RuleField(i, "type"), Filters.TypeName(r.Filter.Type), 70f);
            OptField(cmds, row, RuleField(i, "text"), 170f, r.Filter.Text);
            OptField(cmds, row, RuleField(i, "hue"), 60f, Filters.HueText(r.Filter.Hue));
            OptField(cmds, row, RuleField(i, "color"), 60f, Filters.HueText(r.Color));
            OptButton(cmds, row, RuleField(i, "hide"), r.Hide ? "HIDE" : "show", 44f);
            OptButton(cmds, row, RuleField(i, "del"), "x", 20f);
        }
        OptButton(cmds, OptRow(cmds, root), OptAddRule, "+ Rule", 60f);
    }

    static EntityRef OptRow(Commands cmds, EntityRef parent)
    {
        var n = Node.Base();
        n.FlexDirection = FlexDirection.Row;
        n.AlignItems = AlignItems.Center;
        n.Width = Val.Percent(100f);
        n.Height = Val.Px(OptRowH);
        n.Gap = Val.Px(4f);
        return cmds.Spawn().With(n).ChildOf(parent);
    }

    static void OptLabel(Commands cmds, EntityRef parent, string text, float w, float h, byte grey)
    {
        var n = Node.Base();
        n.Width = Val.Px(w);
        n.Height = Val.Px(h);
        n.AlignItems = AlignItems.Center;
        cmds.Spawn()
            .With(n)
            .With(new Text { Value = text })
            .With(new TextFont { FontId = UiFont, Size = 11 })
            .With(new TextColor { Value = Color.Rgba(grey, grey, (byte)Math.Min(255, grey + 12), 255) })
            .ChildOf(parent);
    }

    // Editable: the host focuses it on press and its editor owns the keys from
    // there; the value comes back through the journal-opt-edit Changed<Text> feed.
    static void OptField(Commands cmds, EntityRef parent, string name, float w, string value)
    {
        var n = Node.Base();
        n.Width = Val.Px(w);
        n.Height = Val.Px(OptRowH - 2f);
        n.AlignItems = AlignItems.Center;
        n.Padding = new UiRect { Left = Val.Px(3f), Right = Val.Px(3f), Top = Val.Px(0f), Bottom = Val.Px(0f) };
        n.Overflow = Overflow.Clip;
        cmds.Spawn(name)
            .With(n)
            .With(new BackgroundColor { Value = Color.Rgba(38, 41, 51, 255) })
            .With(new Text { Value = value })
            .With(new TextFont { FontId = UiFont, Size = 12 })
            .With(new TextColor { Value = Color.Rgba(235, 238, 244, 255) })
            .With<TextInput>()
            .With<EditableText>()
            .With<UiNoWindowDrag>()
            .With(Interaction.None)
            .With(new UiName { Value = name })
            .ChildOf(parent);
    }

    static void OptButton(Commands cmds, EntityRef parent, string name, string text, float w)
    {
        var n = Node.Base();
        n.Width = Val.Px(w);
        n.Height = Val.Px(OptRowH - 2f);
        n.JustifyContent = JustifyContent.Center;
        n.AlignItems = AlignItems.Center;
        cmds.Spawn(name)
            .With(n)
            .With(new BackgroundColor { Value = Color.Rgba(58, 62, 76, 255) })
            .With(BorderRadius.All(3))
            .With(new Text { Value = text })
            .With(new TextFont { FontId = UiFont, Size = 11 })
            .With(new TextColor { Value = Color.Rgba(226, 228, 236, 255) })
            .With(Interaction.None)
            .With<UiNoWindowDrag>()
            .With(new UiName { Value = name })
            .ChildOf(parent);
    }

    void OptClick(Func<string, bool> hit, Commands cmds, ModContext ctx)
    {
        if (hit(OptAddTab))
        {
            if (_tabs.Count < MaxCustomTabs)
            {
                _tabs.Add(new CustomTab());
                TabsChanged(cmds, ctx);
            }
            return;
        }
        if (hit(OptAddRule))
        {
            if (_rules.Count < MaxRules)
            {
                _rules.Add(new Rule());
                _optRebuild = true;
                FiltersChanged(ctx);
            }
            return;
        }

        for (var i = 0; i < _tabs.Count; i++)
        {
            var f = _tabs[i].Filter;
            if (hit(TabField(i, "type")))
            {
                f.Type = Filters.NextType(f.Type);
                Relabel(cmds, ctx, TabField(i, "type"), Filters.TypeName(f.Type));
                FiltersChanged(ctx);
                return;
            }
            if (hit(TabField(i, "del")))
            {
                // Keep the selection on the same tab when an earlier one goes away.
                var idx = TabLabels.Length + i;
                if (_tab == idx) _tab = 0;
                else if (_tab > idx) _tab--;
                _tabs.RemoveAt(i);
                TabsChanged(cmds, ctx);
                return;
            }
        }

        for (var i = 0; i < _rules.Count; i++)
        {
            var r = _rules[i];
            if (hit(RuleField(i, "type")))
            {
                r.Filter.Type = Filters.NextType(r.Filter.Type);
                Relabel(cmds, ctx, RuleField(i, "type"), Filters.TypeName(r.Filter.Type));
                FiltersChanged(ctx);
                return;
            }
            if (hit(RuleField(i, "hide")))
            {
                r.Hide = !r.Hide;
                Relabel(cmds, ctx, RuleField(i, "hide"), r.Hide ? "HIDE" : "show");
                FiltersChanged(ctx);
                return;
            }
            if (hit(RuleField(i, "del")))
            {
                _rules.RemoveAt(i);
                _optRebuild = true;
                FiltersChanged(ctx);
                return;
            }
        }
    }

    void OptEdit(ulong e, string value, Commands cmds, ModContext ctx)
    {
        bool Is(string name) => ctx.Entity(name) == e;
        value = Filters.Clean(value);

        for (var i = 0; i < _tabs.Count; i++)
        {
            var t = _tabs[i];
            if (Is(TabField(i, "name")))
            {
                if (t.Name == value) return;
                t.Name = value;
                var idx = TabLabels.Length + i;
                Relabel(cmds, ctx, TabName(idx), TabLabel(idx));
                Save(ctx);
                return;
            }
            if (Is(TabField(i, "text"))) { t.Filter.Text = value; FiltersChanged(ctx); return; }
            if (Is(TabField(i, "hue"))) { t.Filter.Hue = Filters.ParseHue(value); FiltersChanged(ctx); return; }
        }

        for (var i = 0; i < _rules.Count; i++)
        {
            var r = _rules[i];
            if (Is(RuleField(i, "text"))) { r.Filter.Text = value; FiltersChanged(ctx); return; }
            if (Is(RuleField(i, "hue"))) { r.Filter.Hue = Filters.ParseHue(value); FiltersChanged(ctx); return; }
            if (Is(RuleField(i, "color"))) { r.Color = Filters.ParseHue(value); FiltersChanged(ctx); return; }
        }
    }

    static void Relabel(Commands cmds, ModContext ctx, string name, string text)
    {
        if (ctx.Entity(name) is { } ent)
            cmds.Insert(ent, new Text { Value = text });
    }

    // Filters are applied at paint, so an edit recolours/refilters the history too.
    void FiltersChanged(ModContext ctx)
    {
        _dirty = true;
        Save(ctx);
    }

    // The strip holds one node per tab, so a tab added or removed rebuilds the
    // journal (next tick, from the state just saved) along with the options rows.
    void TabsChanged(Commands cmds, ModContext ctx)
    {
        _optRebuild = true;
        FiltersChanged(ctx);
        Teardown(cmds, ctx);
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

    // Geometry + lock + tab as one CSV line, then the tabs + rules: the SDK's typed
    // storage needs a source-generated JSON context, and this doesn't earn one.
    void LoadState(ModContext ctx)
    {
        var raw = ctx.Storage.GetRaw();
        if (string.IsNullOrEmpty(raw)) return;
        // Window CSV on the first line, tabs + rules after it (Filters.Parse).
        Filters.Parse(raw, _tabs, _rules, MaxCustomTabs, MaxRules);
        var parts = raw.Split('\n')[0].Split(';');
        if (parts.Length < 6) return;
        if (float.TryParse(parts[0], out var x)) _x = x;
        if (float.TryParse(parts[1], out var y)) _y = y;
        if (float.TryParse(parts[2], out var w)) _w = Math.Clamp(w, MinW, MaxW);
        if (float.TryParse(parts[3], out var h)) _h = Math.Clamp(h, MinH, MaxH);
        _locked = parts[4] == "1";
        if (int.TryParse(parts[5], out var tab)) _tab = Math.Clamp(tab, 0, TabCount - 1);
        _savedState = raw;
    }

    void Save(ModContext ctx)
    {
        var blob = $"{_x};{_y};{_w};{_h};{(_locked ? 1 : 0)};{_tab}" + Filters.Serialize(_tabs, _rules);
        if (blob == _savedState) return;
        _savedState = blob;
        ctx.Storage.SetRaw(blob);
    }
}
