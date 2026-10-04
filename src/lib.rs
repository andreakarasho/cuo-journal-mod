// cuo-journal-mod — the ClassicUO 2.0 system-log window, as a mod.
//
// A tabbed, resizable, lockable message log that replaces the client's built-in
// bottom-left scroll — the window root carries `cuo:ui/supersedes`
// ("cuo:ui/system-log"), so the client takes its own log down while this window
// exists and puts it back when it doesn't. No options trip, the two never
// stack, and disabling the mod reverts it with nothing to undo.
//
// Everything it needs is host surface, no bespoke hooks:
//   * lines arrive as cuo:chat/message triggers: kind 1 (the system log channel)
//     and kind 0 (overhead speech), taken the way the client's own journal takes
//     them (filters::journalled / format);
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
// text-contains (case-insensitive) and hue, all optional — filters.rs holds the
// matching and the storage format, SDK-free so `cargo test` can check it.

mod filters;

use cuo_mod_sdk::prelude::*;
use filters::{CustomTab, Filter, Rule};
use serde::Serialize;
use std::cell::RefCell;
use std::rc::Rc;
use types::{align_items, display, flex_direction, justify_content, overflow, val_type, HasPath};
use ui::{BorderRadius, Color, Node, UiRect, Val};

// ---- layout ----------------------------------------------------------
const TAB_H: f32 = 20.0;
const PAD: f32 = 4.0;
const MIN_W: f32 = 140.0;
const MIN_H: f32 = 60.0;
const MAX_W: f32 = 900.0;
const MAX_H: f32 = 700.0;

// The host's flat "modern panel" theme (ClassicUO.Ecs UiTheme.cs): near-black
// background, graphite panels, gold accent, Roboto TTF. Mirrored here because a
// guest can't read the host's palette; keep in step with UiTheme by hand.
mod theme {
    pub type Rgb = (u8, u8, u8);
    pub const BG: Rgb = (18, 18, 18);
    pub const PANEL: Rgb = (30, 30, 36);
    pub const FRAME: Rgb = (70, 70, 80);
    pub const CONTROL: Rgb = (46, 46, 54);
    pub const FIELD: Rgb = (54, 54, 62);
    pub const ACCENT: Rgb = (232, 200, 120);
    pub const WARN: Rgb = (240, 96, 96);
    pub const TEXT: Rgb = (240, 240, 244);
    pub const TEXT_MUTED: Rgb = (170, 172, 180);
    pub const TEXT_ON_ACCENT: Rgb = (20, 20, 24);
    // FontCache face | UoFontRuntime.TtfFlag (0x100); sizes take UiTheme.SizeBoost.
    pub const REGULAR: u16 = 1 | 0x100;
    pub const BOLD: u16 = 2 | 0x100;
    pub const fn size(s: u16) -> u16 {
        s + 2
    }
}

fn rgb(c: theme::Rgb, a: u8) -> Color {
    Color::rgba(c.0, c.1, c.2, a)
}
const LINE_H: f32 = 15.0;
// Bottom-right resize band. The HOST owns the gesture (UiResizable.grip in
// apply_lock) and puts its hit square flush in the corner, so the visible grip
// uses the same size and the same origin — a grip you can see but not grab
// (or the reverse) is worse than none.
const GRIP_SIZE: f32 = 10.0;
// Bit on TextFont.FontId selecting the UO ASCII font set instead of the
// unicode one — the host's UoFontRuntime.AsciiFlag. Server text says which
// set it was sent for, and the built-in log honours it; so does this.
const ASCII_FLAG: u16 = 0x80;
const LINE_SLOTS: usize = 40; // pre-spawned text nodes (the visible window)
const MAX_LINES: usize = 60; // retained history
// How long a line shows while the window is idle. MILLISECONDS: cuo:engine/time
// hands out total as a monotonic ms clock (and frame as seconds — they differ).
// Same 10s the client gives its own log (TIME_DISPLAY_SYSTEM_MESSAGE_TEXT).
const LIFETIME: f32 = 10_000.0;
const FADE_SPEED: f32 = 6.0; // per second
const SCROLL_STEP: i32 = 3; // lines per wheel notch
const IDLE_Z: i32 = 1; // below every gump, like the built-in log
const HOVER_Z: i32 = 30000; // above them while the cursor is on it
// cuo:game/state — 0 Loading, 1 LoginScreen, 2 ServerSelection,
// 3 CharacterSelection, 4 CharacterCreation, 5 LoginError, 6 GameScreen.
const GAME_SCREEN: u8 = 6;

// ---- entity names ----------------------------------------------------
// spawn_named(name) hands the real ecs id back through entity(name) on the NEXT
// call, so nothing is matched on a string at runtime.
const ROOT: &str = "journal.root";
const STRIP: &str = "journal.strip";
const TABS_BOX: &str = "journal.tabs";
const AREA: &str = "journal.area";
const LOCK_BTN: &str = "journal.lock";
const GRIP: &str = "journal.grip";
const OPT_BTN: &str = "journal.optbtn";

// Options window. Separate root: it outlives journal rebuilds (adding a tab
// respawns the journal strip) and never fades.
const OPT: &str = "journal.opt";
const OPT_CLOSE: &str = "journal.opt.close";
const OPT_ADD_TAB: &str = "journal.opt.addtab";
const OPT_ADD_RULE: &str = "journal.opt.addrule";
// ponytail: hard caps instead of a scrolling list, the window just grows.
const MAX_CUSTOM_TABS: usize = 8;
const MAX_RULES: usize = 10;
const OPT_W: f32 = 460.0;
const OPT_ROW_H: f32 = 22.0;
const OPT_CAP_H: f32 = 16.0;
const OPT_GAP: f32 = 4.0;
const OPT_PAD: f32 = 8.0;
const OPT_Z: i32 = HOVER_Z + 1;
const TAB_FIELDS: &[&str] = &["name", "type", "text", "hue", "del"];
const RULE_FIELDS: &[&str] = &["type", "text", "hue", "color", "hide", "del"];

fn tab_field(i: usize, f: &str) -> String {
    format!("journal.opt.t{i}.{f}")
}
fn rule_field(i: usize, f: &str) -> String {
    format!("journal.opt.r{i}.{f}")
}
fn tab_name(i: usize) -> String {
    format!("journal.tab{i}")
}
fn line_name(i: usize) -> String {
    format!("journal.line{i}")
}

const TAB_LABELS: &[&str] = &["All", "Sys", "Chat", "Party", "Guild"];

// Interned type-path ids from the handshake, plus the payload builders over them.
struct Ids {
    node: u16,
    bg: u16,
    radius: u16,
    global_z: u16,
    no_rclose: u16,
    supersedes: u16,
    bounds: u16,
    name: u16,
    text: u16,
    font: u16,
    color: u16,
    hue: u16,
    wrap: u16,
    interaction: u16,
    no_drag: u16,
    movable: u16,
    resizable: u16,
    text_input: u16,
    editable: u16,
    game_state: u16,
    time: u16,
    mouse: u16,
}

impl Ids {
    fn val<T: Serialize>(id: u16, v: &T) -> Comp {
        Comp::value(id, v)
    }
    fn node(&self, n: &Node) -> Comp {
        Self::val(self.node, n)
    }
    fn bg(&self, c: Color) -> Comp {
        Self::val(self.bg, &types::BackgroundColor { value: c })
    }
    fn radius(&self, r: f32) -> Comp {
        Self::val(self.radius, &BorderRadius::all(r))
    }
    fn z(&self, z: i32) -> Comp {
        Self::val(self.global_z, &types::GlobalZIndex { value: z })
    }
    fn name(&self, s: &str) -> Comp {
        Self::val(self.name, &types::UiName { value: s.into() })
    }
    fn text(&self, s: &str) -> Comp {
        Self::val(self.text, &types::Text { value: s.into() })
    }
    fn font(&self, font_id: u16, size: u16) -> Comp {
        Self::val(self.font, &types::TextFont { font_id, size })
    }
    fn color(&self, c: Color) -> Comp {
        Self::val(self.color, &types::TextColor { value: c })
    }
    fn interaction_none(&self) -> Comp {
        Self::val(self.interaction, &types::interaction::NONE)
    }
}

struct Line {
    base_text: String, // as received, without the repeat suffix
    text: String,      // what gets drawn (base_text, or "base_text [N]")
    count: u32,
    tab: usize,   // built-in tab (1..4)
    ty: u8,       // host MessageType, for the filters
    expire: f32,
    hue: u16,     // as the server sent it; rules recolour at paint
    font_id: u16, // UO font id, | ASCII_FLAG when the server sent ascii
}

struct Journal {
    ids: Ids,
    tabs: Vec<CustomTab>,
    rules: Vec<Rule>,
    lines: Vec<Line>,

    spawned: bool,
    loaded: bool,
    dirty: bool, // lines/tab changed — repaint the text column
    last_show_all: bool,
    // Soonest expire among the lines currently on an IDLE window, so the tick can
    // spot "a line just aged out" with one compare. f32::MAX = nothing to wait
    // for (window hovered, or no fresh lines left).
    next_expire: f32,

    x: f32,
    y: f32,
    w: f32,
    h: f32,
    locked: bool,
    tab: usize,
    // Lines between the bottom of the column and the newest line in the tab —
    // 0 = pinned to the newest, which is where an idle window always sits.
    scroll: i32,
    fade: f32,
    was_hovered: bool,
    saved_state: String,
    now: f32,

    // Last values actually pushed to the host. Every write crosses the ABI, so an
    // idle window (the common case) must cost zero commands: paint compares
    // against these and returns early when nothing moved.
    painted_alpha: u8, // != the spawn alpha (0), so the first frame paints
    painted_z: i32,
    painted_tab: usize,
    painted_locked: bool,
    painted_opt: bool,
    painted_w: f32,
    painted_h: f32,

    opt_open: bool,
    opt_spawned: bool,
    opt_rebuild: bool, // add/remove: despawn now, respawn next tick
}

fn setup(m: &mut ModBuilder) {
    let ids = Ids {
        node: m.type_id_of::<types::Node>(),
        bg: m.type_id_of::<types::BackgroundColor>(),
        radius: m.type_id_of::<types::BorderRadius>(),
        global_z: m.type_id_of::<types::GlobalZIndex>(),
        no_rclose: m.type_id_of::<types::UiNoRightClickClose>(),
        supersedes: m.type_id_of::<types::ModSupersedes>(),
        bounds: m.type_id_of::<types::UiContainsByBounds>(),
        name: m.type_id_of::<types::UiName>(),
        text: m.type_id_of::<types::Text>(),
        font: m.type_id_of::<types::TextFont>(),
        color: m.type_id_of::<types::TextColor>(),
        hue: m.type_id_of::<types::TextHue>(),
        wrap: m.type_id_of::<types::TextWrap>(),
        interaction: m.type_id(paths::ui::INTERACTION),
        no_drag: m.type_id_of::<types::UiNoWindowDrag>(),
        movable: m.type_id_of::<types::UiMovable>(),
        resizable: m.type_id_of::<types::UiResizable>(),
        text_input: m.type_id_of::<types::TextInput>(),
        editable: m.type_id_of::<types::EditableText>(),
        game_state: m.type_id_of::<types::GameStateDto>(),
        time: m.type_id_of::<types::Time>(),
        mouse: m.type_id_of::<types::MouseInputDto>(),
    };
    let clicked = m.type_id_of::<types::ModClicked>();
    let owned = m.type_id_of::<types::ModEntity>();
    let (text, editable) = (ids.text, ids.editable);
    let j = Rc::new(RefCell::new(Journal::new(ids)));

    // Host chat -> our list: the system channel (kind 1) AND overhead speech
    // (kind 0), like the client's journal. The host routes each line to exactly
    // one of the two, so nothing lands twice.
    let jj = j.clone();
    m.add_observer(ObserverSpec::on_event(types::ModChatMessage::PATH), move |input, _| {
        let Some(e) = input.value().and_then(|v| v.parse::<types::ModChatMessage>()) else { return };
        if e.text.is_empty() || !filters::journalled(e.kind, e.message_type) {
            return;
        }
        let text = filters::format(e.kind, e.message_type, &e.name, &e.text);
        let tab = filters::tab_of(e.kind, e.message_type);
        jj.borrow_mut().append(text, e.message_type, tab, e.hue, e.font, e.is_unicode);
    });

    let jj = j.clone();
    m.add_system(SystemSpec::new("journal-tick", Schedule::Update).commands(), move |_, cmds| {
        jj.borrow_mut().tick(cmds)
    });

    // Clicks land as the host's one-frame cuo:ui/clicked tag; the entity id
    // alone answers which control it was.
    let jj = j.clone();
    m.add_system(
        SystemSpec::new("journal-clicks", Schedule::Update).commands().query(&[Term::with(clicked)]),
        move |input, cmds| {
            let hits: Vec<u64> = input.query(0).map(|q| q.rows().map(|r| r.entity()).collect()).unwrap_or_default();
            if !hits.is_empty() {
                jj.borrow_mut().clicks(&|name: &str| entity(name).is_some_and(|e| hits.contains(&e)), cmds);
            }
        },
    );

    // Typing in an options field: the host's editor writes the field's Text, so
    // a changed-Text query over our editable nodes is the whole edit feed.
    m.add_system(
        SystemSpec::new("journal-opt-edit", Schedule::Update)
            .commands()
            .query(&[Term::changed(text), Term::with(editable), Term::with(owned)]),
        move |input, cmds| {
            let mut j = j.borrow_mut();
            if !j.opt_spawned {
                return;
            }
            let Some(q) = input.query(0) else { return };
            for row in q.rows() {
                let value = row.comp(0).and_then(|c| c.parse::<types::Text>()).map(|t| t.value).unwrap_or_default();
                j.opt_edit(row.entity(), &value, cmds);
            }
        },
    );
}

export_mod!(setup);

impl Journal {
    fn new(ids: Ids) -> Journal {
        Journal {
            ids,
            tabs: Vec::new(),
            rules: Vec::new(),
            lines: Vec::new(),
            spawned: false,
            loaded: false,
            dirty: true,
            last_show_all: false,
            next_expire: f32::MAX,
            x: 6.0,
            y: 320.0,
            w: 320.0,
            h: 150.0,
            locked: true,
            tab: 0,
            scroll: 0,
            fade: 0.0,
            was_hovered: false,
            saved_state: String::new(),
            now: 0.0,
            painted_alpha: 255,
            painted_z: -1,
            painted_tab: usize::MAX,
            painted_locked: false,
            painted_opt: false,
            painted_w: 0.0,
            painted_h: 0.0,
            opt_open: false,
            opt_spawned: false,
            opt_rebuild: false,
        }
    }

    fn tab_count(&self) -> usize {
        TAB_LABELS.len() + self.tabs.len()
    }

    fn tab_label(&self, i: usize) -> String {
        match TAB_LABELS.get(i) {
            Some(l) => l.to_string(),
            None => match self.tabs[i - TAB_LABELS.len()].name.as_str() {
                "" => "?".into(),
                n => n.into(),
            },
        }
    }

    // ---- ingest ------------------------------------------------------

    fn append(&mut self, text: String, ty: u8, tab: usize, hue: u16, font: u8, is_unicode: bool) {
        // The server picks the font set per message; the built-in log renders
        // each line with the one it was sent for, and so does this.
        let font_id = if is_unicode { font as u16 } else { font as u16 | ASCII_FLAG };
        let expire = self.now + LIFETIME;
        self.dirty = true;

        // Collapse an immediate repeat into "text [N]", like the built-in log —
        // a spammed line shouldn't scroll the window away. Hue/font follow the
        // newest copy, so a repeat that changed colour still reads correctly.
        if let Some(last) = self.lines.last_mut().filter(|l| l.base_text == text) {
            last.count += 1;
            last.text = format!("{text} [{}]", last.count);
            last.hue = hue;
            last.ty = ty;
            last.font_id = font_id;
            last.expire = expire;
            return;
        }
        let line = Line { text: text.clone(), base_text: text, count: 1, tab, ty, expire, hue, font_id };
        // Scrolled back into the history: a new arrival must not shove the view
        // down a line. Only a line this tab shows counts — the others aren't in
        // the column being scrolled. paint_lines clamps, so the MAX_LINES trim
        // below can't leave this pointing past the oldest line.
        if self.scroll > 0 && self.shown(&line) {
            self.scroll += 1;
        }
        if self.lines.len() >= MAX_LINES {
            self.lines.remove(0);
        }
        self.lines.push(line);
    }

    fn matches(f: &Filter, l: &Line) -> bool {
        f.matches(l.ty, &l.base_text, l.hue)
    }

    // In the current tab and not hidden by a rule.
    fn shown(&self, l: &Line) -> bool {
        if self.rules.iter().any(|r| r.hide && Self::matches(&r.filter, l)) {
            return false;
        }
        match self.tab {
            0 => true,
            t if t < TAB_LABELS.len() => l.tab == t,
            t => Self::matches(&self.tabs[t - TAB_LABELS.len()].filter, l),
        }
    }

    // First rule with a colour that matches wins; otherwise the server's hue.
    fn hue_of(&self, l: &Line) -> u16 {
        self.rules
            .iter()
            .find(|r| r.color >= 0 && Self::matches(&r.filter, l))
            .map_or(l.hue, |r| r.color as u16)
    }

    // ---- frame -------------------------------------------------------

    fn tick(&mut self, cmds: &mut CommandBufferBuilder) {
        // A message log belongs to the world, not to the login and character-select
        // screens. The client gates its own the same way — SystemLogGumpPlugin runs
        // its spawn system under RunIf(GameScreen) and despawns OnExit — and a mod
        // has no RunIf, so the check lives here. No answer from the host reads as
        // Loading — which is the reading we want anyway: better no window than one
        // stranded over the login screen.
        let state = imports::resource_get::<types::GameStateDto>(self.ids.game_state).unwrap_or_default();
        if state.current != GAME_SCREEN {
            if self.spawned {
                self.teardown(cmds);
            }
            if self.opt_spawned {
                cmds.despawn_named(OPT);
                self.forget_opt();
            }
            self.opt_open = false;
            return;
        }

        if !self.spawned {
            self.load_state();
            self.spawn(cmds);
            self.spawned = true;
            return;
        }

        let Some(root) = entity(ROOT) else { return };

        // The host owns the window box once it exists (drag + resize write it),
        // so read it back rather than tracking our own copy.
        match imports::component_get::<Node>(root, self.ids.node) {
            Some(node) => {
                self.x = node.left.value;
                self.y = node.top.value;
                self.w = node.width.value;
                self.h = node.height.value;
            }
            None => {
                // No Node on the root we spawned = the host deleted our subtree without
                // telling us. That is what disable→enable does (ModdingPlugin despawns
                // every entity in the mod's slot, then only re-runs ModStartup — and this
                // window is built in Update, not Startup), so without this the mod ticks
                // on forever writing components at a dead id and the window never comes
                // back until a client restart. Reset and let the next tick rebuild it.
                // The options window went down with the rest of the slot.
                self.forget();
                self.forget_opt();
                self.opt_open = false;
                return;
            }
        }

        self.tick_opt(cmds);

        let time = imports::resource_get::<types::Time>(self.ids.time);
        let dt = time.as_ref().map_or(0.016, |t| t.frame); // seconds
        self.now = time.map_or(self.now + dt * 1000.0, |t| t.total); // milliseconds

        if !self.loaded {
            // Storage only answers once the host has registered the mod, which is
            // after mod_setup — so the restore lands on the first tick, not at spawn.
            self.loaded = true;
            self.load_state();
            self.apply_lock(cmds);
            cmds.insert(root, &[self.ids.node(&Node::abs(self.x, self.y, self.w, self.h))]);
        }

        let mouse = imports::resource_get::<types::MouseInputDto>(self.ids.mouse);
        let inside = mouse.as_ref().is_some_and(|m| {
            m.x >= self.x && m.y >= self.y && m.x < self.x + self.w && m.y < self.y + self.h
        });
        // Holding the button after grabbing the window keeps the chrome up while
        // the host drags or resizes it, even when the cursor runs past the edge.
        let hovered = inside || (self.was_hovered && mouse.as_ref().is_some_and(|m| m.left));
        self.was_hovered = hovered;

        // Wheel over the window walks back through the retained history. Hovered
        // only: idle, the window is a notification strip showing the newest lines
        // and nothing else, so it drops back to the bottom when the cursor leaves.
        // cuo:input/mouse hands out the PRE-consume delta in notches (+ up), and
        // plain wheel isn't a client gesture (zoom wants ctrl), so nothing to
        // fight over — and a mod can't consume input anyway.
        let wheel = mouse.as_ref().map_or(0.0, |m| m.wheel);
        if hovered && wheel != 0.0 {
            self.scroll += wheel.round() as i32 * SCROLL_STEP;
            self.dirty = true;
        } else if !hovered && self.scroll != 0 {
            self.scroll = 0;
            self.dirty = true;
        }

        let target = if hovered { 1.0 } else { 0.0 };
        let step = dt * FADE_SPEED;
        self.fade = if (target - self.fade).abs() <= step { target } else { self.fade + step.copysign(target - self.fade) };

        // Drag/resize end: the host writes the box, so persist once the button
        // comes back up (save is a no-op when the blob is unchanged).
        if mouse.as_ref().is_some_and(|m| !m.left) {
            self.save();
        }

        self.paint(cmds);

        let show_all = self.fade >= 1.0;
        // A line ageing out of its lifetime changes what the idle window shows, and
        // it is the one change no event announces — without this the column keeps a
        // dead line on screen until the next message or hover happens to repaint it.
        // paint_lines leaves the next interesting moment behind so this stays a float
        // compare per frame rather than a scan.
        if !show_all && self.now >= self.next_expire {
            self.dirty = true;
        }

        if self.dirty || show_all != self.last_show_all {
            self.last_show_all = show_all;
            self.dirty = false;
            self.paint_lines(cmds, show_all);
        }
    }

    // Leaving the world: take the window down and come back to a clean slate, so
    // logging back in rebuilds it from storage exactly like a fresh login.
    // The retained lines survive — the client keeps its SystemMessages store across
    // the same transition, and a log that forgets on every logout is no log.
    fn teardown(&mut self, cmds: &mut CommandBufferBuilder) {
        cmds.despawn_named(ROOT); // takes the subtree with it, host-side
        self.forget();
    }

    // Drop every binding + cached value tied to a window that no longer exists, so
    // the next tick spawns a fresh one. Split out of teardown because the host can
    // delete the subtree on its own (disable/enable), and then there is nothing left
    // to despawn — only the guest-side bookkeeping to clear.
    fn forget(&mut self) {
        // The subtree despawn frees the ROOT's name only. A child name left pointing
        // at a dead id would, if the host ever recycles ids, have us writing this
        // window's components onto somebody else's entity — so drop every binding.
        // spawn() re-registers all of them. Root included — despawn_named drops its
        // binding, but the host-despawn path never went through it at all.
        for n in [ROOT, STRIP, TABS_BOX, AREA, LOCK_BTN, GRIP, OPT_BTN] {
            forget_entity(n);
        }
        for i in 0..TAB_LABELS.len() + MAX_CUSTOM_TABS {
            forget_entity(&tab_name(i));
        }
        for i in 0..LINE_SLOTS {
            forget_entity(&line_name(i));
        }

        self.spawned = false;
        self.loaded = false; // restore the box + re-apply the lock on respawn
        self.dirty = true;
        self.next_expire = f32::MAX;
        self.scroll = 0;
        self.fade = 0.0;
        self.was_hovered = false;
        // paint short-circuits on "nothing moved", so the cached values have to look
        // impossible or the rebuilt window would keep the old chrome for a frame.
        self.painted_alpha = 255;
        self.painted_z = -1;
        self.painted_tab = usize::MAX;
        self.painted_w = 0.0;
        self.painted_h = 0.0;
    }

    // ---- ui ----------------------------------------------------------

    fn spawn(&self, cmds: &mut CommandBufferBuilder) {
        let ids = &self.ids;
        let root = cmds.spawn_named(ROOT, &[
            ids.node(&Node::abs(self.x, self.y, self.w, self.h)),
            ids.bg(rgb(theme::BG, 0)),
            ids.radius(4.0),
            ids.z(IDLE_Z),
            // Right-click must not close the log; drag/resize are added by
            // apply_lock once the saved lock state is known.
            Comp::marker(ids.no_rclose),
            // THIS window is the system log while it exists: the client hides its
            // own bottom-left scroll and brings it back the moment this entity is
            // gone (mod disabled/unloaded -> host despawns our entities). Live
            // claim, so nothing has to be declared in mod.json or undone on exit.
            Comp::value(ids.supersedes, &types::ModSupersedes { feature: "cuo:ui/system-log".into() }),
            Comp::marker(ids.bounds),
            ids.name(ROOT),
        ]);

        // Tab strip — flow row, so the tabs size to their labels and the lock
        // button rides the right edge at any width.
        let strip = cmds.spawn_named(STRIP, &[
            ids.node(&row(0.0, 0.0, self.w, TAB_H)),
            ids.bg(rgb(theme::PANEL, 0)),
            ids.name(STRIP),
        ]);
        cmds.add_child(root, strip, u32::MAX);

        let tabs = cmds.spawn_named(TABS_BOX, &[ids.node(&grow(TAB_H)), ids.name(TABS_BOX)]);
        cmds.add_child(strip, tabs, u32::MAX);

        for i in 0..self.tab_count() {
            let n = tab_name(i);
            let t = self.strip_button(cmds, &n, &self.tab_label(i), rgb(theme::CONTROL, 0), rgb(theme::TEXT_MUTED, 0));
            cmds.add_child(tabs, t, u32::MAX);
        }
        let opt = self.strip_button(cmds, OPT_BTN, "OPT", rgb(theme::CONTROL, 0), rgb(theme::TEXT_MUTED, 0));
        cmds.add_child(strip, opt, u32::MAX);
        let lock = self.strip_button(cmds, LOCK_BTN, "LOCK", rgb(theme::CONTROL, 0), rgb(theme::ACCENT, 0));
        cmds.add_child(strip, lock, u32::MAX);

        // Text column: bottom-aligned, clipped, so lines stack upward from the
        // bottom edge like the built-in log.
        let area = cmds.spawn_named(AREA, &[
            ids.node(&column(PAD, TAB_H, self.w - PAD * 2.0, self.h - TAB_H - 2.0)),
            ids.name(AREA),
        ]);
        cmds.add_child(root, area, u32::MAX);

        for i in 0..LINE_SLOTS {
            let n = line_name(i);
            let l = cmds.spawn_named(&n, &[
                ids.node(&line_node()),
                ids.text(""),
                ids.font(1, 13), // UO unicode font 1 placeholder; paint_lines sets the server's
                ids.color(rgb(theme::TEXT, 255)),
                Comp::value(ids.wrap, &types::TextWrap { kind: types::text_wrap_kind::WORDS }),
                ids.name(&n),
            ]);
            cmds.add_child(area, l, u32::MAX);
        }

        // Resize grip. Purely a handle to look at — apply_lock's UiResizable is
        // what actually resizes, and the host's grab band is this same corner
        // square. Chrome, and unlocked-only: a locked window can't be resized,
        // so it must not advertise a grip (paint drives both).
        let grip = cmds.spawn_named(GRIP, &[
            ids.node(&grip_node(self.w, self.h)),
            ids.bg(rgb(theme::FRAME, 0)),
            ids.radius(3.0),
            ids.name(GRIP),
        ]);
        cmds.add_child(root, grip, u32::MAX);
    }

    fn strip_button(&self, cmds: &mut CommandBufferBuilder, name: &str, label: &str, bg: Color, fg: Color) -> TempId {
        let ids = &self.ids;
        cmds.spawn_named(name, &[
            ids.node(&tab_node()),
            ids.bg(bg),
            ids.radius(3.0),
            ids.text(label),
            ids.font(theme::BOLD, theme::size(11)),
            ids.color(fg),
            ids.interaction_none(),
            Comp::marker(ids.no_drag),
            ids.name(name),
        ])
    }

    // Per-frame repaint of everything the fade touches. Colours only — the
    // geometry belongs to the host once the window is up.
    fn paint(&mut self, cmds: &mut CommandBufferBuilder) {
        let alpha = self.alpha(255);
        if alpha == self.painted_alpha
            && self.tab == self.painted_tab
            && self.locked == self.painted_locked
            && self.opt_open == self.painted_opt
            && self.w == self.painted_w
            && self.h == self.painted_h
        {
            return;
        }
        let size_changed = self.w != self.painted_w || self.h != self.painted_h;
        self.painted_alpha = alpha;
        self.painted_tab = self.tab;
        self.painted_locked = self.locked;
        self.painted_opt = self.opt_open;
        self.painted_w = self.w;
        self.painted_h = self.h;
        let ids = &self.ids;
        let fade = self.fade;
        let a = move |full: u8| alpha_of(full, fade);

        if let Some(root) = entity(ROOT) {
            cmds.insert(root, &[ids.bg(rgb(theme::BG, a(200)))]);
            // Idle the log sits UNDER every gump (like the built-in one); hovering
            // lifts it so the tabs aren't buried by whatever is on top.
            let z = if alpha > 0 { HOVER_Z } else { IDLE_Z };
            if z != self.painted_z {
                self.painted_z = z;
                cmds.insert(root, &[ids.z(z)]);
            }
        }

        if let Some(strip) = entity(STRIP) {
            let mut r = row(0.0, 0.0, self.w, TAB_H);
            r.display = if self.fade > 0.01 { display::FLEX } else { display::NONE };
            cmds.insert(strip, &[ids.bg(rgb(theme::PANEL, a(245))), ids.node(&r)]);
        }

        // Selected reads like the host's: gold fill, dark caption.
        let (on_bg, on_fg) = (rgb(theme::ACCENT, a(255)), rgb(theme::TEXT_ON_ACCENT, a(255)));
        let (off_bg, off_fg) = (rgb(theme::CONTROL, a(255)), rgb(theme::TEXT_MUTED, a(255)));
        for i in 0..self.tab_count() {
            let Some(tab) = entity(&tab_name(i)) else { continue };
            let active = i == self.tab;
            cmds.insert(tab, &[
                ids.bg(if active { on_bg.clone() } else { off_bg.clone() }),
                ids.color(if active { on_fg.clone() } else { off_fg.clone() }),
            ]);
        }

        if let Some(opt) = entity(OPT_BTN) {
            let open = self.opt_open;
            cmds.insert(opt, &[
                ids.color(if open { on_fg.clone() } else { off_fg.clone() }),
                ids.bg(if open { on_bg.clone() } else { off_bg.clone() }),
            ]);
        }

        if let Some(lock) = entity(LOCK_BTN) {
            // Locked reads gold (accent), unlocked warn-red — same convention the built-in
            // window uses for its padlock (the gump set has no open-padlock art).
            let locked = self.locked;
            cmds.insert(lock, &[
                ids.text(if locked { "LOCK" } else { "MOVE" }),
                ids.color(if locked { rgb(theme::ACCENT, a(255)) } else { rgb(theme::WARN, a(255)) }),
                ids.bg(off_bg.clone()),
            ]);
        }

        // The text column tracks the window box (the host resizes the ROOT only;
        // absolute children keep their own size). Only on a real resize.
        if size_changed {
            if let Some(area) = entity(AREA) {
                cmds.insert(area, &[ids.node(&column(PAD, TAB_H, self.w - PAD * 2.0, self.h - TAB_H - 2.0))]);
            }
        }

        if let Some(grip) = entity(GRIP) {
            let mut g = grip_node(self.w, self.h);
            g.display = if alpha > 0 && !self.locked { display::FLEX } else { display::NONE };
            cmds.insert(grip, &[ids.bg(rgb(theme::FRAME, a(255))), ids.node(&g)]);
        }
    }

    fn alpha(&self, full: u8) -> u8 {
        alpha_of(full, self.fade)
    }

    fn paint_lines(&mut self, cmds: &mut CommandBufferBuilder, show_all: bool) {
        // Newest last: walk the tail of the list that passes the tab + freshness
        // filter, oldest first, and fill the slots bottom-up.
        //
        // Along the way, remember when the SHOWN set next changes by itself — the
        // soonest expiry among the lines actually on screen. tick watches that one
        // value instead of re-scanning, and while hovered nothing expires out of
        // view at all, so there is nothing to watch.
        let mut next_expire = f32::MAX;

        // Scroll offset, clamped against what this tab actually holds — the wheel
        // is free to run past either end, and one line always stays on screen so
        // the window never goes blank. 60 lines, so counting beats tracking it.
        let matching = self.lines.iter().filter(|l| self.shown(l)).count() as i32;
        self.scroll = self.scroll.clamp(0, (matching - 1).max(0));
        // Idle shows the newest lines and only those: there is nothing to scroll
        // when what you can see is "whatever arrived in the last 10 seconds".
        let mut skip = if show_all { self.scroll } else { 0 };

        let mut picked: Vec<&Line> = Vec::with_capacity(LINE_SLOTS);
        for line in self.lines.iter().rev() {
            if picked.len() >= LINE_SLOTS {
                break;
            }
            if !self.shown(line) {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if !show_all {
                if line.expire <= self.now {
                    continue;
                }
                next_expire = next_expire.min(line.expire);
            }
            picked.push(line);
        }
        picked.reverse();

        let first = LINE_SLOTS - picked.len();
        for slot in 0..LINE_SLOTS {
            let Some(ent) = entity(&line_name(slot)) else { continue };
            let mut node = line_node();
            let Some(line) = slot.checked_sub(first).map(|i| picked[i]) else {
                node.display = display::NONE;
                cmds.insert(ent, &[self.ids.node(&node)]);
                continue;
            };
            // Hand the host the RAW server hue and let it paint the line exactly as
            // the built-in log does — baked colour, legacy black border, and the
            // ascii/unicode split resolved from FontId. Converting the hue here
            // instead would be both flatter (no border) and wrong: hue_color answers
            // GetPolygoneColor(30, hue + 1), the wire convention chat and nameplates
            // use, while text baking answers GetPolygoneColor(30, hue) — one table
            // entry apart. Letting the host do it also drops a guest round-trip per
            // line, so there is nothing left to cache.
            cmds.insert(ent, &[
                self.ids.node(&node),
                self.ids.text(&line.text),
                self.ids.font(line.font_id, 13),
                Comp::value(self.ids.hue, &types::TextHue { value: self.hue_of(line) }),
            ]);
        }
        self.next_expire = next_expire;
    }

    // ---- options window ----------------------------------------------

    fn tick_opt(&mut self, cmds: &mut CommandBufferBuilder) {
        if self.opt_rebuild {
            self.opt_rebuild = false;
            if self.opt_spawned {
                // Despawn now, spawn next tick: a name can't be re-bound in the same
                // frame that frees it.
                cmds.despawn_named(OPT);
                self.forget_opt();
                return;
            }
        }
        if self.opt_open && !self.opt_spawned {
            self.spawn_opt(cmds);
            self.opt_spawned = true;
        }
    }

    fn forget_opt(&mut self) {
        for n in [OPT, OPT_CLOSE, OPT_ADD_TAB, OPT_ADD_RULE] {
            forget_entity(n);
        }
        for i in 0..MAX_CUSTOM_TABS {
            TAB_FIELDS.iter().for_each(|f| forget_entity(&tab_field(i, f)));
        }
        for i in 0..MAX_RULES {
            RULE_FIELDS.iter().for_each(|f| forget_entity(&rule_field(i, f)));
        }
        self.opt_spawned = false;
    }

    fn spawn_opt(&self, cmds: &mut CommandBufferBuilder) {
        let ids = &self.ids;
        // Children: title, caption, tab rows, add, caption, rule rows, add.
        let rows = (self.tabs.len() + self.rules.len()) as f32;
        let h = OPT_PAD * 2.0 + OPT_ROW_H * (rows + 3.0) + OPT_CAP_H * 2.0 + OPT_GAP * (rows + 4.0);
        let bx = Node {
            flex_direction: flex_direction::COLUMN,
            padding: UiRect::splat(Val::px(OPT_PAD)),
            gap: Val::px(OPT_GAP),
            ..Node::abs(self.x, (self.y - h - 4.0).max(0.0), OPT_W, h)
        };
        let root = cmds.spawn_named(OPT, &[
            ids.node(&bx),
            ids.bg(rgb(theme::BG, 250)),
            ids.radius(4.0),
            ids.z(OPT_Z),
            Comp::marker(ids.movable),
            Comp::marker(ids.no_rclose),
            Comp::marker(ids.bounds),
            ids.interaction_none(),
            ids.name(OPT),
        ]);

        let title = self.opt_row(cmds, root);
        self.opt_label(cmds, title, "Journal options", OPT_W - OPT_PAD * 2.0 - 28.0, OPT_ROW_H, true);
        self.opt_button(cmds, title, OPT_CLOSE, "X", 20.0);

        self.opt_label(cmds, root, "Tabs:  name · type · text contains · hue", OPT_W, OPT_CAP_H, false);
        for (i, t) in self.tabs.iter().enumerate() {
            let row = self.opt_row(cmds, root);
            self.opt_field(cmds, row, &tab_field(i, "name"), 90.0, &t.name);
            self.opt_button(cmds, row, &tab_field(i, "type"), &filters::type_name(t.filter.ty), 70.0);
            self.opt_field(cmds, row, &tab_field(i, "text"), 170.0, &t.filter.text);
            self.opt_field(cmds, row, &tab_field(i, "hue"), 60.0, &filters::hue_text(t.filter.hue));
            self.opt_button(cmds, row, &tab_field(i, "del"), "x", 20.0);
        }
        let add = self.opt_row(cmds, root);
        self.opt_button(cmds, add, OPT_ADD_TAB, "+ Tab", 60.0);

        self.opt_label(cmds, root, "Rules:  type · text contains · hue · new hue · hide", OPT_W, OPT_CAP_H, false);
        for (i, r) in self.rules.iter().enumerate() {
            let row = self.opt_row(cmds, root);
            self.opt_button(cmds, row, &rule_field(i, "type"), &filters::type_name(r.filter.ty), 70.0);
            self.opt_field(cmds, row, &rule_field(i, "text"), 170.0, &r.filter.text);
            self.opt_field(cmds, row, &rule_field(i, "hue"), 60.0, &filters::hue_text(r.filter.hue));
            self.opt_field(cmds, row, &rule_field(i, "color"), 60.0, &filters::hue_text(r.color));
            self.opt_button(cmds, row, &rule_field(i, "hide"), if r.hide { "HIDE" } else { "show" }, 44.0);
            self.opt_button(cmds, row, &rule_field(i, "del"), "x", 20.0);
        }
        let add = self.opt_row(cmds, root);
        self.opt_button(cmds, add, OPT_ADD_RULE, "+ Rule", 60.0);
    }

    fn opt_row(&self, cmds: &mut CommandBufferBuilder, parent: TempId) -> TempId {
        let n = Node {
            flex_direction: flex_direction::ROW,
            align_items: align_items::CENTER,
            width: Val::percent(100.0),
            height: Val::px(OPT_ROW_H),
            gap: Val::px(4.0),
            ..Node::base()
        };
        let r = cmds.spawn_with(&[self.ids.node(&n)]);
        cmds.add_child(parent, r, u32::MAX);
        r
    }

    fn opt_label(&self, cmds: &mut CommandBufferBuilder, parent: TempId, text: &str, w: f32, h: f32, title: bool) {
        let n = Node { width: Val::px(w), height: Val::px(h), align_items: align_items::CENTER, ..Node::base() };
        let l = cmds.spawn_with(&[
            self.ids.node(&n),
            self.ids.text(text),
            if title { self.ids.font(theme::BOLD, theme::size(13)) } else { self.ids.font(theme::REGULAR, theme::size(11)) },
            self.ids.color(rgb(if title { theme::TEXT } else { theme::TEXT_MUTED }, 255)),
        ]);
        cmds.add_child(parent, l, u32::MAX);
    }

    // Editable: the host focuses it on press and its editor owns the keys from
    // there; the value comes back through the journal-opt-edit changed-Text feed.
    fn opt_field(&self, cmds: &mut CommandBufferBuilder, parent: TempId, name: &str, w: f32, value: &str) {
        let ids = &self.ids;
        let n = Node {
            width: Val::px(w),
            height: Val::px(OPT_ROW_H - 2.0),
            align_items: align_items::CENTER,
            padding: pad_x(3.0),
            overflow: overflow::CLIP,
            ..Node::base()
        };
        let f = cmds.spawn_named(name, &[
            ids.node(&n),
            ids.bg(rgb(theme::FIELD, 255)),
            ids.radius(3.0),
            ids.text(value),
            ids.font(theme::REGULAR, theme::size(12)),
            ids.color(rgb(theme::TEXT, 255)),
            Comp::marker(ids.text_input),
            Comp::marker(ids.editable),
            Comp::marker(ids.no_drag),
            ids.interaction_none(),
            ids.name(name),
        ]);
        cmds.add_child(parent, f, u32::MAX);
    }

    fn opt_button(&self, cmds: &mut CommandBufferBuilder, parent: TempId, name: &str, text: &str, w: f32) {
        let ids = &self.ids;
        let n = Node {
            width: Val::px(w),
            height: Val::px(OPT_ROW_H - 2.0),
            justify_content: justify_content::CENTER,
            align_items: align_items::CENTER,
            ..Node::base()
        };
        let b = cmds.spawn_named(name, &[
            ids.node(&n),
            ids.bg(rgb(theme::CONTROL, 255)),
            ids.radius(3.0),
            ids.text(text),
            ids.font(theme::REGULAR, theme::size(11)),
            ids.color(rgb(theme::TEXT, 255)),
            ids.interaction_none(),
            Comp::marker(ids.no_drag),
            ids.name(name),
        ]);
        cmds.add_child(parent, b, u32::MAX);
    }

    fn clicks(&mut self, hit: &dyn Fn(&str) -> bool, cmds: &mut CommandBufferBuilder) {
        if !self.spawned {
            return;
        }

        for i in 0..self.tab_count() {
            if !hit(&tab_name(i)) {
                continue;
            }
            if self.tab != i {
                self.tab = i;
                self.scroll = 0; // a different channel, a different history
                self.dirty = true;
                self.save();
            }
            return;
        }

        if hit(LOCK_BTN) {
            self.locked = !self.locked;
            self.apply_lock(cmds);
            self.save();
            return;
        }

        if hit(OPT_BTN) || (self.opt_spawned && hit(OPT_CLOSE)) {
            self.opt_open = !self.opt_open;
            self.opt_rebuild = true;
            return;
        }

        if self.opt_spawned {
            self.opt_click(hit, cmds);
        }
    }

    fn opt_click(&mut self, hit: &dyn Fn(&str) -> bool, cmds: &mut CommandBufferBuilder) {
        if hit(OPT_ADD_TAB) {
            if self.tabs.len() < MAX_CUSTOM_TABS {
                self.tabs.push(CustomTab::default());
                self.tabs_changed(cmds);
            }
            return;
        }
        if hit(OPT_ADD_RULE) {
            if self.rules.len() < MAX_RULES {
                self.rules.push(Rule::default());
                self.opt_rebuild = true;
                self.filters_changed();
            }
            return;
        }

        for i in 0..self.tabs.len() {
            if hit(&tab_field(i, "type")) {
                let f = &mut self.tabs[i].filter;
                f.ty = filters::next_type(f.ty);
                let label = filters::type_name(f.ty);
                self.relabel(cmds, &tab_field(i, "type"), &label);
                self.filters_changed();
                return;
            }
            if hit(&tab_field(i, "del")) {
                // Keep the selection on the same tab when an earlier one goes away.
                let idx = TAB_LABELS.len() + i;
                if self.tab == idx {
                    self.tab = 0;
                } else if self.tab > idx {
                    self.tab -= 1;
                }
                self.tabs.remove(i);
                self.tabs_changed(cmds);
                return;
            }
        }

        for i in 0..self.rules.len() {
            if hit(&rule_field(i, "type")) {
                let f = &mut self.rules[i].filter;
                f.ty = filters::next_type(f.ty);
                let label = filters::type_name(f.ty);
                self.relabel(cmds, &rule_field(i, "type"), &label);
                self.filters_changed();
                return;
            }
            if hit(&rule_field(i, "hide")) {
                let r = &mut self.rules[i];
                r.hide = !r.hide;
                let label = if r.hide { "HIDE" } else { "show" };
                self.relabel(cmds, &rule_field(i, "hide"), label);
                self.filters_changed();
                return;
            }
            if hit(&rule_field(i, "del")) {
                self.rules.remove(i);
                self.opt_rebuild = true;
                self.filters_changed();
                return;
            }
        }
    }

    fn opt_edit(&mut self, e: u64, value: &str, cmds: &mut CommandBufferBuilder) {
        let is = |name: String| entity(&name) == Some(e);
        let value = filters::clean(value);

        for i in 0..self.tabs.len() {
            if is(tab_field(i, "name")) {
                if self.tabs[i].name == value {
                    return;
                }
                self.tabs[i].name = value;
                let idx = TAB_LABELS.len() + i;
                self.relabel(cmds, &tab_name(idx), &self.tab_label(idx));
                self.save();
                return;
            }
            let f = &mut self.tabs[i].filter;
            if is(tab_field(i, "text")) {
                f.text = value;
            } else if is(tab_field(i, "hue")) {
                f.hue = filters::parse_hue(&value);
            } else {
                continue;
            }
            return self.filters_changed();
        }

        for i in 0..self.rules.len() {
            let r = &mut self.rules[i];
            if is(rule_field(i, "text")) {
                r.filter.text = value;
            } else if is(rule_field(i, "hue")) {
                r.filter.hue = filters::parse_hue(&value);
            } else if is(rule_field(i, "color")) {
                r.color = filters::parse_hue(&value);
            } else {
                continue;
            }
            return self.filters_changed();
        }
    }

    fn relabel(&self, cmds: &mut CommandBufferBuilder, name: &str, text: &str) {
        if let Some(ent) = entity(name) {
            cmds.insert(ent, &[self.ids.text(text)]);
        }
    }

    // Filters are applied at paint, so an edit recolours/refilters the history too.
    fn filters_changed(&mut self) {
        self.dirty = true;
        self.save();
    }

    // The strip holds one node per tab, so a tab added or removed rebuilds the
    // journal (next tick, from the state just saved) along with the options rows.
    fn tabs_changed(&mut self, cmds: &mut CommandBufferBuilder) {
        self.opt_rebuild = true;
        self.filters_changed();
        self.teardown(cmds);
    }

    // ---- lock / persistence -----------------------------------------

    // Locked = no drag, no resize. Both gestures are host-owned markers, so this
    // is just add/remove.
    fn apply_lock(&self, cmds: &mut CommandBufferBuilder) {
        let Some(root) = entity(ROOT) else { return };
        let ids = &self.ids;
        if self.locked {
            cmds.remove(root, &[ids.movable, ids.resizable]);
            return;
        }
        cmds.insert(root, &[
            Comp::marker(ids.movable),
            Comp::value(ids.resizable, &types::UiResizable {
                min_w: MIN_W,
                min_h: MIN_H,
                max_w: MAX_W,
                max_h: MAX_H,
                grip: GRIP_SIZE, // same square the visible grip draws
            }),
        ]);
    }

    // Geometry + lock + tab as one CSV line, then the tabs + rules (filters::parse).
    // Same blob the C# build wrote, so an existing storage.json carries over.
    fn load_state(&mut self) {
        let raw = storage::get();
        if raw.is_empty() {
            return;
        }
        filters::parse(&raw, &mut self.tabs, &mut self.rules, MAX_CUSTOM_TABS, MAX_RULES);
        let parts: Vec<&str> = raw.split('\n').next().unwrap_or("").split(';').collect();
        if parts.len() < 6 {
            return;
        }
        let num = |s: &str| s.trim().parse::<f32>().ok();
        if let Some(x) = num(parts[0]) { self.x = x; }
        if let Some(y) = num(parts[1]) { self.y = y; }
        if let Some(w) = num(parts[2]) { self.w = w.clamp(MIN_W, MAX_W); }
        if let Some(h) = num(parts[3]) { self.h = h.clamp(MIN_H, MAX_H); }
        self.locked = parts[4] == "1";
        if let Ok(t) = parts[5].trim().parse::<i64>() {
            self.tab = t.clamp(0, self.tab_count() as i64 - 1) as usize;
        }
        self.saved_state = raw;
    }

    fn save(&mut self) {
        let blob = format!(
            "{};{};{};{};{};{}{}",
            self.x, self.y, self.w, self.h, self.locked as u8, self.tab,
            filters::serialize(&self.tabs, &self.rules)
        );
        if blob != self.saved_state {
            storage::set(&blob);
            self.saved_state = blob;
        }
    }
}

// ---- node shapes -------------------------------------------------------

fn alpha_of(full: u8, fade: f32) -> u8 {
    (full as f32 * fade).clamp(0.0, 255.0) as u8
}

fn pad_x(v: f32) -> UiRect {
    UiRect { left: Val::px(v), right: Val::px(v), top: Val::px(0.0), bottom: Val::px(0.0) }
}

// Flush to the bottom-right corner — UiResizablePlugin's hit square is
// (left + width - grip, top + height - grip), so no inset here either.
fn grip_node(w: f32, h: f32) -> Node {
    Node::abs(w - GRIP_SIZE, h - GRIP_SIZE, GRIP_SIZE, GRIP_SIZE)
}

fn row(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node {
        flex_direction: flex_direction::ROW,
        align_items: align_items::CENTER,
        gap: Val::px(2.0),
        padding: pad_x(2.0),
        ..Node::abs(x, y, w, h)
    }
}

fn grow(h: f32) -> Node {
    Node {
        flex_direction: flex_direction::ROW,
        align_items: align_items::CENTER,
        overflow: overflow::CLIP,
        // Fill what the parent has left (Clay Grow).
        width: Val { type_: val_type::GROW, value: 0.0, is_auto: false },
        height: Val::px(h),
        gap: Val::px(2.0),
        ..Node::base()
    }
}

fn tab_node() -> Node {
    Node {
        height: Val::px(TAB_H - 2.0),
        justify_content: justify_content::CENTER,
        align_items: align_items::CENTER,
        padding: pad_x(5.0),
        ..Node::base()
    }
}

fn column(x: f32, y: f32, w: f32, h: f32) -> Node {
    Node {
        flex_direction: flex_direction::COLUMN,
        justify_content: justify_content::END,
        overflow: overflow::CLIP,
        ..Node::abs(x, y, w, h)
    }
}

fn line_node() -> Node {
    Node { width: Val::percent(100.0), height: Val::auto(), min_height: Val::px(LINE_H), ..Node::base() }
}
