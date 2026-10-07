// cuo-journal-mod — the ClassicUO 2.0 system-log window, as a mod.
//
// A tabbed, resizable, lockable message log that replaces the client's built-in
// bottom-left scroll — the window root carries `cuo:ui/supersedes`
// ("cuo:ui/system-log"), so the client takes its own log down while this window
// exists and puts it back when it doesn't. No options trip, the two never
// stack, and disabling the mod reverts it with nothing to undo.
//
// Everything it needs is host surface, no bespoke hooks:
//   * lines arrive as cuo:chat/message events (observed): kind 1 (the system log
//     channel) and kind 0 (overhead speech), taken the way the client's own
//     journal takes them (filters::journalled / format);
//   * the window is plain cuo:ui/* nodes parented with cuo:ecs/child-of — no
//     custom rendering;
//   * lines carry cuo:ui/text-hue, so the host paints them exactly the way it
//     paints its own log: the server's hue baked in, legacy black border, and
//     the ascii/unicode font split taken from the hue the server sent;
//   * dragging is cuo:ui/movable, resizing is cuo:ui/resizable (the host owns
//     both gestures — a mod must never do rect math off the raw mouse), and
//   * cuo:ui/no-right-click-close keeps a stray right-click from closing it, and
//     cuo:ui/supersedes is how it claims the host's system-log feature;
//   * buttons answer the cuo:ui/click event; option fields are fed by the host's
//     editor (a Changed<Text> query over editable nodes).
//
// Idle the window is invisible: the panel fades out and only the text is left,
// and since mod nodes carry no UiCustom they are never a hit target, so clicks
// fall through to the world exactly like the built-in log. Hovering (or holding
// a drag) fades the panel + chrome back in and shows the full retained history
// instead of just the lines still inside their 10s.
//
// The OPT button on the strip opens the options window: custom tabs (a name + a
// filter) and rules (a filter + a new hue and/or hide). A filter is message type,
// text-contains (case-insensitive) and hue, all optional — filters.rs holds the
// matching and the storage format, SDK-free so `cargo test` can check it.

mod filters;

use cuo_mod_sdk::host::Scope;
use cuo_mod_sdk::prelude::*;
use filters::{CustomTab, Filter, Rule};
use std::cell::RefCell;

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
// Frames a freshly spawned root may stay out of the Node query before it counts
// as lost. It lands the frame after the spawn; this is slack.
const ROOT_GRACE: u32 = 60;

// ---- entity names ----------------------------------------------------
// cuo:ui/name tags — identity for the host's `-layout` inspector (and whoever
// checks the window in-world). The mod itself keeps the spawned entities.
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

// What a control in the options window is.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Field {
    Close,
    AddTab,
    AddRule,
    TabName(usize),
    TabType(usize),
    TabText(usize),
    TabHue(usize),
    TabDel(usize),
    RuleType(usize),
    RuleText(usize),
    RuleHue(usize),
    RuleColor(usize),
    RuleHide(usize),
    RuleDel(usize),
}

// The journal window's entities, as spawned.
struct Ui {
    root: Entity,
    strip: Entity,
    area: Entity,
    lock: Entity,
    grip: Entity,
    opt_btn: Entity,
    tabs: Vec<Entity>,
    lines: Vec<Entity>,
    // The root has shown up in the Node query; from then on, missing means the
    // host deleted it.
    seen: bool,
    waited: u32,
}

// The options window's entities.
struct OptUi {
    root: Entity,
    fields: Vec<(Entity, Field)>,
}

impl OptUi {
    fn field_of(&self, e: Entity) -> Option<Field> {
        self.fields.iter().find(|f| f.0 == e).map(|f| f.1)
    }
    fn entity_of(&self, f: Field) -> Option<Entity> {
        self.fields.iter().find(|x| x.1 == f).map(|x| x.0)
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
    tabs: Vec<CustomTab>,
    rules: Vec<Rule>,
    lines: Vec<Line>,

    ui: Option<Ui>,
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
    opt: Option<OptUi>,
    opt_rebuild: bool, // add/remove: despawn now, respawn next tick
}

thread_local! {
    // The one journal, shared by the systems and observers below (single-threaded
    // guest; the host never runs two of them at once).
    static JOURNAL: RefCell<Journal> = RefCell::new(Journal::new());
}

fn with<R>(f: impl FnOnce(&mut Journal) -> R) -> R {
    JOURNAL.with(|j| f(&mut j.borrow_mut()))
}

fn setup(app: &mut App) {
    app.add_systems(Schedule::Update, (tick, opt_edit.after(tick)));
    app.add_observer(on_chat);
    app.add_observer(on_click);
}

export_mod!(setup);

// Host chat -> our list: the system channel (kind 1) AND overhead speech (kind 0),
// like the client's journal. The host routes each line to exactly one of the two,
// so nothing lands twice.
fn on_chat(e: On<Event, ChatMessage>) {
    if e.text.is_empty() || !filters::journalled(e.kind, e.message_type) {
        return;
    }
    let text = filters::format(e.kind, e.message_type, &e.name, &e.text);
    let tab = filters::tab_of(e.kind, e.message_type);
    with(|j| j.append(text, e.message_type, tab, e.hue, e.font, e.is_unicode));
}

// The window box. Of our entities only the root carries the supersedes claim.
type Roots = Query<&'static Node, With<ModSupersedes>>;

fn tick(
    state: Option<Res<GameStateDto>>,
    time: Option<Res<Time>>,
    mouse: Option<Res<MouseInputDto>>,
    roots: Roots,
    mut cmds: Commands,
) {
    // No answer from the host reads as Loading — better no window than one
    // stranded over the login screen.
    let in_game = state.is_some_and(|s| s.current == GAME_SCREEN);
    let time = time.map(|t| (t.total, t.frame));
    let mouse = mouse.map(|m| m.clone());
    with(|j| j.tick(in_game, time, mouse.as_ref(), &roots, &mut cmds));
}

// Clicks arrive as the host's cuo:ui/click event on the clicked entity; the entity
// alone answers which control it was.
fn on_click(click: On<Event, UiClick>, mut cmds: Commands) {
    with(|j| j.clicks(click.entity(), &mut cmds));
}

// Typing in an options field: the host's editor writes the field's Text, so a
// changed-Text query over editable nodes is the whole edit feed.
fn opt_edit(fields: Query<&Text, (With<EditableText>, Changed<Text>)>, mut cmds: Commands) {
    with(|j| {
        if j.opt.is_none() {
            return;
        }
        for (e, t) in &fields {
            j.opt_edit(e, &t.value, &mut cmds);
        }
    });
}

impl Journal {
    fn new() -> Journal {
        Journal {
            tabs: Vec::new(),
            rules: Vec::new(),
            lines: Vec::new(),
            ui: None,
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
            opt: None,
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

    fn tick(
        &mut self,
        in_game: bool,
        time: Option<(f32, f32)>,
        mouse: Option<&MouseInputDto>,
        roots: &Roots,
        cmds: &mut Commands,
    ) {
        // A message log belongs to the world, not to the login and character-select
        // screens. The client gates its own the same way — SystemLogGumpPlugin runs
        // its spawn system under RunIf(GameScreen) and despawns OnExit — and a mod
        // has no RunIf, so the check lives here.
        if !in_game {
            if self.ui.is_some() {
                self.teardown(cmds);
            }
            if let Some(opt) = self.opt.take() {
                cmds.entity(opt.root).despawn();
            }
            self.opt_open = false;
            return;
        }

        let Some(ui) = self.ui.as_mut() else {
            self.load_state();
            self.spawn(cmds);
            return;
        };

        // The host owns the window box once it exists (drag + resize write it),
        // so read it back rather than tracking our own copy.
        let root = ui.root;
        match roots.get(root) {
            Some(node) => {
                ui.seen = true;
                self.x = node.left.value;
                self.y = node.top.value;
                self.w = node.width.value;
                self.h = node.height.value;
            }
            None => {
                ui.waited += 1;
                if !ui.seen && ui.waited <= ROOT_GRACE {
                    return; // the spawn hasn't landed yet
                }
                if !ui.seen {
                    cmds.entity(root).despawn();
                }
                // A root we saw is gone = the host deleted our subtree without
                // telling us. That is what disable→enable does (the host despawns
                // every entity the mod owns, then only re-runs Startup — and this
                // window is built in Update), so without this the mod ticks on
                // forever writing components at a dead id and the window never
                // comes back until a client restart. Reset and let the next tick
                // rebuild it. The options window went down with the rest.
                self.forget();
                self.opt = None;
                self.opt_open = false;
                return;
            }
        }

        self.tick_opt(cmds);

        let dt = time.map_or(0.016, |t| t.1); // seconds
        self.now = time.map_or(self.now + dt * 1000.0, |t| t.0); // milliseconds

        if !self.loaded {
            // Storage only answers once the host has registered the mod, which is
            // after mod_setup — so the restore lands on the first tick, not at spawn.
            self.loaded = true;
            self.load_state();
            self.apply_lock(cmds);
            cmds.entity(root).insert(Node::abs(self.x, self.y, self.w, self.h));
        }

        let inside = mouse.is_some_and(|m| {
            m.x >= self.x && m.y >= self.y && m.x < self.x + self.w && m.y < self.y + self.h
        });
        // Holding the button after grabbing the window keeps the chrome up while
        // the host drags or resizes it, even when the cursor runs past the edge.
        let hovered = inside || (self.was_hovered && mouse.is_some_and(|m| m.left));
        self.was_hovered = hovered;

        // Wheel over the window walks back through the retained history. Hovered
        // only: idle, the window is a notification strip showing the newest lines
        // and nothing else, so it drops back to the bottom when the cursor leaves.
        // cuo:input/mouse hands out the PRE-consume delta in notches (+ up), and
        // plain wheel isn't a client gesture (zoom wants ctrl), so nothing to
        // fight over.
        let wheel = mouse.map_or(0.0, |m| m.wheel);
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
        if mouse.is_some_and(|m| !m.left) {
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
    fn teardown(&mut self, cmds: &mut Commands) {
        if let Some(ui) = &self.ui {
            cmds.entity(ui.root).despawn(); // takes the subtree with it, host-side
        }
        self.forget();
    }

    // Drop everything tied to a window that no longer exists, so the next tick
    // spawns a fresh one. Split out of teardown because the host can delete the
    // subtree on its own (disable/enable), and then there is nothing left to
    // despawn — only the guest-side bookkeeping to clear.
    fn forget(&mut self) {
        self.ui = None;
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

    fn spawn(&mut self, cmds: &mut Commands) {
        let root = cmds
            .spawn((
                Node::abs(self.x, self.y, self.w, self.h),
                BackgroundColor { value: rgb(theme::BG, 0) },
                BorderRadius::all(4.0),
                GlobalZIndex { value: IDLE_Z },
                // Right-click must not close the log; drag/resize are added by
                // apply_lock once the saved lock state is known.
                UiNoRightClickClose {},
                // THIS window is the system log while it exists: the client hides its
                // own bottom-left scroll and brings it back the moment this entity is
                // gone (mod disabled/unloaded -> host despawns our entities). Live
                // claim, so nothing has to be declared in mod.json or undone on exit.
                ModSupersedes { feature: "cuo:ui/system-log".into() },
                UiContainsByBounds {},
                UiName { value: ROOT.into() },
            ))
            .id();

        // Tab strip — flow row, so the tabs size to their labels and the lock
        // button rides the right edge at any width.
        let strip = cmds
            .spawn((
                row(0.0, 0.0, self.w, TAB_H),
                BackgroundColor { value: rgb(theme::PANEL, 0) },
                UiName { value: STRIP.into() },
                ChildOf::new(root),
            ))
            .id();

        let tabs_box = cmds.spawn((grow(TAB_H), UiName { value: TABS_BOX.into() }, ChildOf::new(strip))).id();

        let mut tabs = Vec::with_capacity(self.tab_count());
        for i in 0..self.tab_count() {
            let label = self.tab_label(i);
            let (bg, fg) = (rgb(theme::CONTROL, 0), rgb(theme::TEXT_MUTED, 0));
            tabs.push(strip_button(cmds, tabs_box, &tab_name(i), &label, bg, fg));
        }
        let opt_btn = strip_button(cmds, strip, OPT_BTN, "OPT", rgb(theme::CONTROL, 0), rgb(theme::TEXT_MUTED, 0));
        let lock = strip_button(cmds, strip, LOCK_BTN, "LOCK", rgb(theme::CONTROL, 0), rgb(theme::ACCENT, 0));

        // Text column: bottom-aligned, clipped, so lines stack upward from the
        // bottom edge like the built-in log.
        let area = cmds
            .spawn((
                column(PAD, TAB_H, self.w - PAD * 2.0, self.h - TAB_H - 2.0),
                UiName { value: AREA.into() },
                ChildOf::new(root),
            ))
            .id();

        let mut lines = Vec::with_capacity(LINE_SLOTS);
        for i in 0..LINE_SLOTS {
            let line = cmds.spawn((
                line_node(),
                Text::default(),
                // UO unicode font 1 placeholder; paint_lines sets the server's.
                TextFont { font_id: 1, size: 13 },
                TextColor { value: rgb(theme::TEXT, 255) },
                TextWrap { kind: text_wrap_kind::WORDS },
                UiName { value: line_name(i) },
                ChildOf::new(area),
            ));
            lines.push(line.id());
        }

        // Resize grip. Purely a handle to look at — apply_lock's UiResizable is
        // what actually resizes, and the host's grab band is this same corner
        // square. Chrome, and unlocked-only: a locked window can't be resized,
        // so it must not advertise a grip (paint drives both).
        let grip = cmds
            .spawn((
                grip_node(self.w, self.h),
                BackgroundColor { value: rgb(theme::FRAME, 0) },
                BorderRadius::all(3.0),
                UiName { value: GRIP.into() },
                ChildOf::new(root),
            ))
            .id();

        self.ui = Some(Ui { root, strip, area, lock, grip, opt_btn, tabs, lines, seen: false, waited: 0 });
    }

    // Per-frame repaint of everything the fade touches. Colours only — the
    // geometry belongs to the host once the window is up.
    fn paint(&mut self, cmds: &mut Commands) {
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
        let fade = self.fade;
        let a = move |full: u8| alpha_of(full, fade);

        // Idle the log sits UNDER every gump (like the built-in one); hovering
        // lifts it so the tabs aren't buried by whatever is on top.
        let z = if alpha > 0 { HOVER_Z } else { IDLE_Z };
        let z_changed = z != self.painted_z;
        self.painted_z = z;

        let Some(ui) = &self.ui else { return };
        cmds.entity(ui.root).insert(BackgroundColor { value: rgb(theme::BG, a(200)) });
        if z_changed {
            cmds.entity(ui.root).insert(GlobalZIndex { value: z });
        }

        let mut r = row(0.0, 0.0, self.w, TAB_H);
        r.display = if self.fade > 0.01 { display::FLEX } else { display::NONE };
        cmds.entity(ui.strip).insert((BackgroundColor { value: rgb(theme::PANEL, a(245)) }, r));

        // Selected reads like the host's: gold fill, dark caption.
        let (on_bg, on_fg) = (rgb(theme::ACCENT, a(255)), rgb(theme::TEXT_ON_ACCENT, a(255)));
        let (off_bg, off_fg) = (rgb(theme::CONTROL, a(255)), rgb(theme::TEXT_MUTED, a(255)));
        for (i, &tab) in ui.tabs.iter().enumerate() {
            let active = i == self.tab;
            cmds.entity(tab).insert((
                BackgroundColor { value: if active { on_bg.clone() } else { off_bg.clone() } },
                TextColor { value: if active { on_fg.clone() } else { off_fg.clone() } },
            ));
        }

        let open = self.opt_open;
        cmds.entity(ui.opt_btn).insert((
            TextColor { value: if open { on_fg.clone() } else { off_fg.clone() } },
            BackgroundColor { value: if open { on_bg.clone() } else { off_bg.clone() } },
        ));

        // Locked reads gold (accent), unlocked warn-red — same convention the built-in
        // window uses for its padlock (the gump set has no open-padlock art).
        let locked = self.locked;
        cmds.entity(ui.lock).insert((
            Text { value: if locked { "LOCK" } else { "MOVE" }.into() },
            TextColor { value: if locked { rgb(theme::ACCENT, a(255)) } else { rgb(theme::WARN, a(255)) } },
            BackgroundColor { value: off_bg.clone() },
        ));

        // The text column tracks the window box (the host resizes the ROOT only;
        // absolute children keep their own size). Only on a real resize.
        if size_changed {
            cmds.entity(ui.area).insert(column(PAD, TAB_H, self.w - PAD * 2.0, self.h - TAB_H - 2.0));
        }

        let mut g = grip_node(self.w, self.h);
        g.display = if alpha > 0 && !self.locked { display::FLEX } else { display::NONE };
        cmds.entity(ui.grip).insert((BackgroundColor { value: rgb(theme::FRAME, a(255)) }, g));
    }

    fn alpha(&self, full: u8) -> u8 {
        alpha_of(full, self.fade)
    }

    fn paint_lines(&mut self, cmds: &mut Commands, show_all: bool) {
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

        if let Some(ui) = &self.ui {
            let first = LINE_SLOTS - picked.len();
            for (slot, &ent) in ui.lines.iter().enumerate() {
                let mut node = line_node();
                let Some(line) = slot.checked_sub(first).map(|i| picked[i]) else {
                    node.display = display::NONE;
                    cmds.entity(ent).insert(node);
                    continue;
                };
                // Hand the host the RAW server hue and let it paint the line exactly
                // as the built-in log does — baked colour, legacy black border, and
                // the ascii/unicode split resolved from FontId. Converting the hue
                // here instead would be both flatter (no border) and wrong: a hue
                // ramp lookup answers GetPolygoneColor(30, hue + 1), the wire
                // convention chat and nameplates use, while text baking answers
                // GetPolygoneColor(30, hue) — one table entry apart.
                cmds.entity(ent).insert((
                    node,
                    Text { value: line.text.clone() },
                    TextFont { font_id: line.font_id, size: 13 },
                    TextHue { value: self.hue_of(line) },
                ));
            }
        }
        self.next_expire = next_expire;
    }

    // ---- options window ----------------------------------------------

    fn tick_opt(&mut self, cmds: &mut Commands) {
        if self.opt_rebuild {
            self.opt_rebuild = false;
            if let Some(opt) = self.opt.take() {
                // Despawn now, spawn next tick, so no click or edit aimed at the old
                // rows can land on the new ones.
                cmds.entity(opt.root).despawn();
                return;
            }
        }
        if self.opt_open && self.opt.is_none() {
            self.spawn_opt(cmds);
        }
    }

    fn spawn_opt(&mut self, cmds: &mut Commands) {
        // Children: title, caption, tab rows, add, caption, rule rows, add.
        let rows = (self.tabs.len() + self.rules.len()) as f32;
        let h = OPT_PAD * 2.0 + OPT_ROW_H * (rows + 3.0) + OPT_CAP_H * 2.0 + OPT_GAP * (rows + 4.0);
        let bx = Node {
            flex_direction: flex_direction::COLUMN,
            padding: UiRect::splat(Val::px(OPT_PAD)),
            gap: Val::px(OPT_GAP),
            ..Node::abs(self.x, (self.y - h - 4.0).max(0.0), OPT_W, h)
        };
        let root = cmds
            .spawn((
                bx,
                BackgroundColor { value: rgb(theme::BG, 250) },
                BorderRadius::all(4.0),
                GlobalZIndex { value: OPT_Z },
                UiMovable {},
                UiNoRightClickClose {},
                UiContainsByBounds {},
                Interaction::NONE,
                UiName { value: OPT.into() },
            ))
            .id();
        let mut fields = Vec::new();
        let mut field = |e: Entity, f: Field| fields.push((e, f));

        let title = opt_row(cmds, root);
        opt_label(cmds, title, "Journal options", OPT_W - OPT_PAD * 2.0 - 28.0, OPT_ROW_H, true);
        field(opt_button(cmds, title, OPT_CLOSE, "X", 20.0), Field::Close);

        opt_label(cmds, root, "Tabs:  name · type · text contains · hue", OPT_W, OPT_CAP_H, false);
        for (i, t) in self.tabs.iter().enumerate() {
            let row = opt_row(cmds, root);
            let ty = filters::type_name(t.filter.ty);
            field(opt_field(cmds, row, &tab_field(i, "name"), 90.0, &t.name), Field::TabName(i));
            field(opt_button(cmds, row, &tab_field(i, "type"), &ty, 70.0), Field::TabType(i));
            field(opt_field(cmds, row, &tab_field(i, "text"), 170.0, &t.filter.text), Field::TabText(i));
            field(opt_field(cmds, row, &tab_field(i, "hue"), 60.0, &filters::hue_text(t.filter.hue)), Field::TabHue(i));
            field(opt_button(cmds, row, &tab_field(i, "del"), "x", 20.0), Field::TabDel(i));
        }
        let add = opt_row(cmds, root);
        field(opt_button(cmds, add, OPT_ADD_TAB, "+ Tab", 60.0), Field::AddTab);

        opt_label(cmds, root, "Rules:  type · text contains · hue · new hue · hide", OPT_W, OPT_CAP_H, false);
        for (i, r) in self.rules.iter().enumerate() {
            let row = opt_row(cmds, root);
            let ty = filters::type_name(r.filter.ty);
            let hide = if r.hide { "HIDE" } else { "show" };
            field(opt_button(cmds, row, &rule_field(i, "type"), &ty, 70.0), Field::RuleType(i));
            field(opt_field(cmds, row, &rule_field(i, "text"), 170.0, &r.filter.text), Field::RuleText(i));
            field(opt_field(cmds, row, &rule_field(i, "hue"), 60.0, &filters::hue_text(r.filter.hue)), Field::RuleHue(i));
            field(opt_field(cmds, row, &rule_field(i, "color"), 60.0, &filters::hue_text(r.color)), Field::RuleColor(i));
            field(opt_button(cmds, row, &rule_field(i, "hide"), hide, 44.0), Field::RuleHide(i));
            field(opt_button(cmds, row, &rule_field(i, "del"), "x", 20.0), Field::RuleDel(i));
        }
        let add = opt_row(cmds, root);
        field(opt_button(cmds, add, OPT_ADD_RULE, "+ Rule", 60.0), Field::AddRule);

        self.opt = Some(OptUi { root, fields });
    }

    fn clicks(&mut self, e: Entity, cmds: &mut Commands) {
        let Some(ui) = &self.ui else { return };

        if let Some(i) = ui.tabs.iter().position(|&t| t == e) {
            if self.tab != i {
                self.tab = i;
                self.scroll = 0; // a different channel, a different history
                self.dirty = true;
                self.save();
            }
            return;
        }

        if e == ui.lock {
            self.locked = !self.locked;
            self.apply_lock(cmds);
            self.save();
            return;
        }

        let field = self.opt.as_ref().and_then(|o| o.field_of(e));
        if e == ui.opt_btn || field == Some(Field::Close) {
            self.opt_open = !self.opt_open;
            self.opt_rebuild = true;
            return;
        }

        if let Some(f) = field {
            self.opt_click(f, cmds);
        }
    }

    fn opt_click(&mut self, f: Field, cmds: &mut Commands) {
        match f {
            Field::AddTab => {
                if self.tabs.len() < MAX_CUSTOM_TABS {
                    self.tabs.push(CustomTab::default());
                    self.tabs_changed(cmds);
                }
            }
            Field::AddRule => {
                if self.rules.len() < MAX_RULES {
                    self.rules.push(Rule::default());
                    self.opt_rebuild = true;
                    self.filters_changed();
                }
            }
            Field::TabType(i) if i < self.tabs.len() => {
                let fl = &mut self.tabs[i].filter;
                fl.ty = filters::next_type(fl.ty);
                let label = filters::type_name(fl.ty);
                self.relabel_field(cmds, f, &label);
                self.filters_changed();
            }
            Field::TabDel(i) if i < self.tabs.len() => {
                // Keep the selection on the same tab when an earlier one goes away.
                let idx = TAB_LABELS.len() + i;
                if self.tab == idx {
                    self.tab = 0;
                } else if self.tab > idx {
                    self.tab -= 1;
                }
                self.tabs.remove(i);
                self.tabs_changed(cmds);
            }
            Field::RuleType(i) if i < self.rules.len() => {
                let fl = &mut self.rules[i].filter;
                fl.ty = filters::next_type(fl.ty);
                let label = filters::type_name(fl.ty);
                self.relabel_field(cmds, f, &label);
                self.filters_changed();
            }
            Field::RuleHide(i) if i < self.rules.len() => {
                let r = &mut self.rules[i];
                r.hide = !r.hide;
                let label = if r.hide { "HIDE" } else { "show" };
                self.relabel_field(cmds, f, label);
                self.filters_changed();
            }
            Field::RuleDel(i) if i < self.rules.len() => {
                self.rules.remove(i);
                self.opt_rebuild = true;
                self.filters_changed();
            }
            _ => {}
        }
    }

    fn opt_edit(&mut self, e: Entity, value: &str, cmds: &mut Commands) {
        let Some(f) = self.opt.as_ref().and_then(|o| o.field_of(e)) else { return };
        let value = filters::clean(value);

        match f {
            Field::TabName(i) if i < self.tabs.len() => {
                if self.tabs[i].name == value {
                    return;
                }
                self.tabs[i].name = value;
                let idx = TAB_LABELS.len() + i;
                let label = self.tab_label(idx);
                if let Some(&tab) = self.ui.as_ref().and_then(|ui| ui.tabs.get(idx)) {
                    cmds.entity(tab).insert(Text { value: label });
                }
                self.save();
                return;
            }
            Field::TabText(i) if i < self.tabs.len() => self.tabs[i].filter.text = value,
            Field::TabHue(i) if i < self.tabs.len() => self.tabs[i].filter.hue = filters::parse_hue(&value),
            Field::RuleText(i) if i < self.rules.len() => self.rules[i].filter.text = value,
            Field::RuleHue(i) if i < self.rules.len() => self.rules[i].filter.hue = filters::parse_hue(&value),
            Field::RuleColor(i) if i < self.rules.len() => self.rules[i].color = filters::parse_hue(&value),
            _ => return,
        }
        self.filters_changed();
    }

    fn relabel_field(&self, cmds: &mut Commands, f: Field, text: &str) {
        if let Some(e) = self.opt.as_ref().and_then(|o| o.entity_of(f)) {
            cmds.entity(e).insert(Text { value: text.into() });
        }
    }

    // Filters are applied at paint, so an edit recolours/refilters the history too.
    fn filters_changed(&mut self) {
        self.dirty = true;
        self.save();
    }

    // The strip holds one node per tab, so a tab added or removed rebuilds the
    // journal (next tick, from the state just saved) along with the options rows.
    fn tabs_changed(&mut self, cmds: &mut Commands) {
        self.opt_rebuild = true;
        self.filters_changed();
        self.teardown(cmds);
    }

    // ---- lock / persistence -----------------------------------------

    // Locked = no drag, no resize. Both gestures are host-owned markers, so this
    // is just add/remove.
    fn apply_lock(&self, cmds: &mut Commands) {
        let Some(ui) = &self.ui else { return };
        if self.locked {
            cmds.entity(ui.root).remove::<(UiMovable, UiResizable)>();
            return;
        }
        cmds.entity(ui.root).insert((
            UiMovable {},
            UiResizable {
                min_w: MIN_W,
                min_h: MIN_H,
                max_w: MAX_W,
                max_h: MAX_H,
                grip: GRIP_SIZE, // same square the visible grip draws
                ..Default::default()
            },
        ));
    }

    // Geometry + lock + tab as one CSV line, then the tabs + rules (filters::parse).
    // Same blob the C# build wrote, so an existing storage.json carries over.
    fn load_state(&mut self) {
        let raw = host::storage_get(Scope::Global);
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
            host::storage_set(Scope::Global, &blob);
            self.saved_state = blob;
        }
    }
}

// ---- spawners ----------------------------------------------------------

fn strip_button(cmds: &mut Commands, parent: Entity, name: &str, label: &str, bg: Color, fg: Color) -> Entity {
    cmds.spawn((
        tab_node(),
        BackgroundColor { value: bg },
        BorderRadius::all(3.0),
        Text { value: label.into() },
        TextFont { font_id: theme::BOLD, size: theme::size(11) },
        TextColor { value: fg },
        Interaction::NONE,
        UiNoWindowDrag {},
        UiName { value: name.into() },
        ChildOf::new(parent),
    ))
    .id()
}

fn opt_row(cmds: &mut Commands, parent: Entity) -> Entity {
    let n = Node {
        flex_direction: flex_direction::ROW,
        align_items: align_items::CENTER,
        width: Val::percent(100.0),
        height: Val::px(OPT_ROW_H),
        gap: Val::px(4.0),
        ..Node::base()
    };
    cmds.spawn((n, ChildOf::new(parent))).id()
}

fn opt_label(cmds: &mut Commands, parent: Entity, text: &str, w: f32, h: f32, title: bool) {
    let n = Node { width: Val::px(w), height: Val::px(h), align_items: align_items::CENTER, ..Node::base() };
    let font = if title {
        TextFont { font_id: theme::BOLD, size: theme::size(13) }
    } else {
        TextFont { font_id: theme::REGULAR, size: theme::size(11) }
    };
    cmds.spawn((
        n,
        Text { value: text.into() },
        font,
        TextColor { value: rgb(if title { theme::TEXT } else { theme::TEXT_MUTED }, 255) },
        ChildOf::new(parent),
    ));
}

// Editable: the host focuses it on press and its editor owns the keys from
// there; the value comes back through the opt_edit changed-Text feed.
fn opt_field(cmds: &mut Commands, parent: Entity, name: &str, w: f32, value: &str) -> Entity {
    let n = Node {
        width: Val::px(w),
        height: Val::px(OPT_ROW_H - 2.0),
        align_items: align_items::CENTER,
        padding: pad_x(3.0),
        overflow: overflow::CLIP,
        ..Node::base()
    };
    cmds.spawn((
        n,
        BackgroundColor { value: rgb(theme::FIELD, 255) },
        BorderRadius::all(3.0),
        Text { value: value.into() },
        TextFont { font_id: theme::REGULAR, size: theme::size(12) },
        TextColor { value: rgb(theme::TEXT, 255) },
        TextInput {},
        EditableText::default(),
        UiNoWindowDrag {},
        Interaction::NONE,
        UiName { value: name.into() },
        ChildOf::new(parent),
    ))
    .id()
}

fn opt_button(cmds: &mut Commands, parent: Entity, name: &str, text: &str, w: f32) -> Entity {
    let n = Node {
        width: Val::px(w),
        height: Val::px(OPT_ROW_H - 2.0),
        justify_content: justify_content::CENTER,
        align_items: align_items::CENTER,
        ..Node::base()
    };
    cmds.spawn((
        n,
        BackgroundColor { value: rgb(theme::CONTROL, 255) },
        BorderRadius::all(3.0),
        Text { value: text.into() },
        TextFont { font_id: theme::REGULAR, size: theme::size(11) },
        TextColor { value: rgb(theme::TEXT, 255) },
        Interaction::NONE,
        UiNoWindowDrag {},
        UiName { value: name.into() },
        ChildOf::new(parent),
    ))
    .id()
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
