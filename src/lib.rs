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
//   * the window is an HTML/CSS page (journal.html / journal.css, <body data-theme>),
//     so the client's UI theme draws it and a player restyles it; the mod fills it
//     by element id and reads its buttons with page-clicks;
//   * a line carries its font (`uo-N` / `uo-ascii-N`) and `color: hue(N)` inline, so
//     the host paints it the way it paints its own log: the server's hue baked in,
//     legacy black border, and the ascii/unicode font split the server sent;
//   * dragging is cuo:ui/movable, resizing is cuo:ui/resizable (the host owns
//     both gestures — a mod must never do rect math off the raw mouse), and
//   * cuo:ui/no-right-click-close keeps a stray right-click from closing it, and
//     cuo:ui/supersedes is how it claims the host's system-log feature.
//
// Idle the window is invisible: the body gets `.idle` (no panel, no chrome) and only
// the text is left, and locked the root is no movable window, so clicks fall through
// to the world exactly like the built-in log. Hovering (or holding a drag) brings the
// panel + chrome back and shows the full retained history instead of just the lines
// still inside their 10s.
//
// The OPT button on the strip opens the options window (options.html / options.css):
// custom tabs (a name + a filter) and rules (a filter + a new hue and/or hide). A
// filter is message type, text-contains (case-insensitive) and hue, all optional —
// filters.rs holds the matching and the storage format, SDK-free so `cargo test`
// can check it.

mod filters;

use cuo_mod_sdk::host::{PageChange, Scope};
use cuo_mod_sdk::prelude::*;
use filters::{CustomTab, Filter, Rule};
use std::cell::RefCell;

// The pages, exported once to Data/Mods/journal/ui/ where the player's copy wins.
const PAGE: &str = "journal";
const PAGE_HTML: &str = include_str!("journal.html");
const PAGE_CSS: &str = include_str!("journal.css");
const OPT_PAGE: &str = "options";
const OPT_HTML: &str = include_str!("options.html");
const OPT_CSS: &str = include_str!("options.css");

const MIN_W: f32 = 140.0;
const MIN_H: f32 = 60.0;
const MAX_W: f32 = 900.0;
const MAX_H: f32 = 700.0;
// Bottom-right resize band. The HOST owns the gesture (UiResizable.grip in
// apply_lock) and puts its hit square flush in the corner, so the page's #grip
// is the same size at the same origin — a grip you can see but not grab
// (or the reverse) is worse than none.
const GRIP_SIZE: f32 = 10.0;
const LINE_SLOTS: usize = 40; // most lines on the page at once (the visible window)
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

// cuo:ui/name tags — identity for the host's `-layout` inspector.
const ROOT: &str = "journal.root";
const OPT: &str = "journal.opt";

// ponytail: hard caps instead of a scrolling list, the window just grows.
const MAX_CUSTOM_TABS: usize = 8;
const MAX_RULES: usize = 10;
// The options page's height without rows and per row, to open it just above the
// journal. An estimate of options.css: the page sizes itself.
const OPT_H: f32 = 150.0;
const OPT_ROW_H: f32 = 26.0;
const OPT_Z: i32 = HOVER_Z + 1;
// The inputs of an options row, read as #t<i>-<field> and #r<i>-<field>.
const TAB_FIELDS: [&str; 3] = ["name", "text", "hue"];
const RULE_FIELDS: [&str; 3] = ["text", "hue", "color"];

const TAB_LABELS: &[&str] = &["All", "Sys", "Chat", "Party", "Guild"];

struct Line {
    base_text: String, // as received, without the repeat suffix
    text: String,      // what gets drawn (base_text, or "base_text [N]")
    count: u32,
    tab: usize,   // built-in tab (1..4)
    ty: u8,       // host MessageType, for the filters
    expire: f32,
    hue: u16,     // as the server sent it; rules recolour at paint
    font: String, // the CSS family of the UO font the server sent it for
}

struct Journal {
    tabs: Vec<CustomTab>,
    rules: Vec<Rule>,
    lines: Vec<Line>,

    page: Option<u64>, // the journal window
    loaded: bool,
    dirty: bool, // lines/tab changed — refill #lines
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

    // (hovered, tab, locked, options open) as last pushed to the host. Every write
    // crosses the ABI, so an idle window (the common case) must cost zero calls.
    painted: Option<(bool, usize, bool, bool)>,
    tabs_dirty: bool, // a tab was added, removed or renamed — refill #tabs

    opt_open: bool,
    opt: Option<u64>, // the options window
    opt_rows: u64,    // its #tab-rows element as last seen: a reload makes a new one
    opt_dirty: bool,  // refill its rows
    // The inputs' values at the last read, in TAB_FIELDS / RULE_FIELDS order. Empty
    // after a refill: the next read is the baseline, not an edit.
    opt_seen: Vec<String>,
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
    app.add_systems(Schedule::Update, tick);
    app.add_observer(on_chat);
}

export_mod!(setup);

// Host chat -> our list: the system channel (kind 1) AND overhead speech (kind 0),
// like the client's journal. The host routes each line to exactly one of the two,
// so nothing lands twice.
#[system]
fn on_chat(e: On<Event, ChatMessage>) {
    if e.text.is_empty() || !filters::journalled(e.kind, e.message_type) {
        return;
    }
    let text = filters::format(e.kind, e.message_type, &e.name, &e.text);
    let tab = filters::tab_of(e.kind, e.message_type);
    with(|j| j.append(text, e.message_type, tab, e.hue, e.font, e.is_unicode));
}

// The window box. Of our entities only the journal's root carries the supersedes claim.
type Roots = Query<&'static Node, With<ModSupersedes>>;

#[system]
fn tick(
    state: Option<Res<GameState>>,
    time: Option<Res<Time>>,
    mouse: Option<Res<MouseInput>>,
    // Spelled out, not `Roots`: #[system] reads the parameter kind from the type name.
    roots: Query<&'static Node, With<ModSupersedes>>,
    mut cmds: Commands,
) {
    // No answer from the host reads as Loading — better no window than one
    // stranded over the login screen.
    let in_game = state.is_some_and(|s| s.current == GAME_SCREEN);
    let time = time.map(|t| (t.total, t.frame));
    let mouse = mouse.map(|m| m.clone());
    with(|j| j.tick(in_game, time, mouse.as_ref(), &roots, &mut cmds));
}

impl Journal {
    fn new() -> Journal {
        Journal {
            tabs: Vec::new(),
            rules: Vec::new(),
            lines: Vec::new(),
            page: None,
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
            painted: None,
            tabs_dirty: true,
            opt_open: false,
            opt: None,
            opt_rows: 0,
            opt_dirty: false,
            opt_seen: Vec::new(),
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
        let font = if is_unicode { format!("uo-{font}") } else { format!("uo-ascii-{font}") };
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
            last.font = font;
            last.expire = expire;
            return;
        }
        let line = Line { text: text.clone(), base_text: text, count: 1, tab, ty, expire, hue, font };
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
        mouse: Option<&MouseInput>,
        roots: &Roots,
        cmds: &mut Commands,
    ) {
        // A message log belongs to the world, not to the login and character-select
        // screens. The client gates its own the same way — SystemLogGumpPlugin runs
        // its spawn system under RunIf(GameScreen) and despawns OnExit — and a mod
        // has no RunIf, so the check lives here.
        if !in_game {
            self.teardown();
            return;
        }

        let Some(page) = self.page else {
            self.load_state();
            self.spawn(cmds);
            return;
        };

        // A window that is gone = the host deleted it without telling us. That is
        // what disable→enable does (the host despawns every entity the mod owns,
        // then only re-runs Startup — and this window is built in Update), so
        // without this the mod ticks on forever editing a dead page and the window
        // never comes back until a client restart. Reset and let the next tick
        // rebuild it. The options window went down with the rest.
        if !host::page_is_open(page) {
            self.forget();
            return;
        }

        // The host owns the window box once it exists (drag + resize write it),
        // so read it back rather than tracking our own copy. Not in the query the
        // frame of the spawn: the saved box stands until it is.
        if let Some(node) = roots.get(Entity::from_bits(page)) {
            self.x = node.left.value;
            self.y = node.top.value;
            self.w = node.width.value;
            self.h = node.height.value;
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
            cmds.entity(Entity::from_bits(page)).insert(self.root_node());
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

        // The page fades its own panel (a CSS transition); this is only the time
        // the history waits before it replaces the fresh lines.
        let target = if hovered { 1.0 } else { 0.0 };
        let step = dt * FADE_SPEED;
        self.fade = if (target - self.fade).abs() <= step { target } else { self.fade + step.copysign(target - self.fade) };

        // Drag/resize end: the host writes the box, so persist once the button
        // comes back up (save is a no-op when the blob is unchanged).
        if mouse.is_some_and(|m| !m.left) {
            self.save();
        }

        for click in host::page_clicks(page) {
            self.click(&click, cmds);
        }
        self.paint(page, hovered, cmds);

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
            self.paint_lines(page, show_all);
        }
    }

    // Leaving the world: take the windows down and come back to a clean slate, so
    // logging back in rebuilds the journal from storage exactly like a fresh login.
    // The retained lines survive — the client keeps its SystemMessages store across
    // the same transition, and a log that forgets on every logout is no log.
    fn teardown(&mut self) {
        for page in [self.page, self.opt].into_iter().flatten() {
            host::page_close(page);
        }
        self.forget();
    }

    // Drop everything tied to windows that no longer exist, so the next tick
    // spawns a fresh journal. Split out of teardown because the host can delete
    // them on its own (disable/enable), and then there is nothing left to close —
    // only the guest-side bookkeeping to clear.
    fn forget(&mut self) {
        self.page = None;
        self.opt = None;
        self.opt_open = false;
        self.loaded = false; // restore the box + re-apply the lock on respawn
        self.dirty = true;
        self.next_expire = f32::MAX;
        self.scroll = 0;
        self.fade = 0.0;
        self.was_hovered = false;
        // paint short-circuits on "nothing moved": a new page has nothing of ours yet.
        self.painted = None;
        self.tabs_dirty = true;
    }

    // ---- ui ----------------------------------------------------------

    // The window box: the page's body fills it.
    fn root_node(&self) -> Node {
        let mut n = Node::abs(self.x, self.y, self.w, self.h);
        n.display = display::FLEX;
        n
    }

    fn spawn(&mut self, cmds: &mut Commands) {
        let Some(page) = host::page_spawn(PAGE, PAGE_HTML, PAGE_CSS, self.x as i32, self.y as i32) else { return };
        cmds.entity(Entity::from_bits(page)).insert((
            self.root_node(),
            GlobalZIndex { value: IDLE_Z },
            // Right-click must not close the log.
            UiNoRightClickClose {},
            // THIS window is the system log while it exists: the client hides its
            // own bottom-left scroll and brings it back the moment this entity is
            // gone (mod disabled/unloaded -> host despawns our entities). Live
            // claim, so nothing has to be declared in mod.json or undone on exit.
            ModSupersedes { feature: "cuo:ui/system-log".into() },
            UiContainsByBounds {},
            UiName { value: ROOT.into() },
        ));
        self.page = Some(page);
        // A page's window is born movable: a locked log must not be, from the start.
        self.apply_lock(cmds);
    }

    // The chrome: classes and captions only — the page owns the look, and the
    // geometry belongs to the host once the window is up.
    fn paint(&mut self, page: u64, hovered: bool, cmds: &mut Commands) {
        let state = (hovered, self.tab, self.locked, self.opt_open);
        if self.painted == Some(state) && !self.tabs_dirty {
            return;
        }
        let old = self.painted.replace(state);

        // Idle the log sits UNDER every gump (like the built-in one); hovering
        // lifts it so the tabs aren't buried by whatever is on top.
        if old.map(|o| o.0) != Some(hovered) {
            cmds.entity(Entity::from_bits(page)).insert(GlobalZIndex { value: if hovered { HOVER_Z } else { IDLE_Z } });
        }
        if self.tabs_dirty || old.map(|o| o.1) != Some(self.tab) {
            self.tabs_dirty = false;
            let tabs: String = (0..self.tab_count()).map(|i| tab_html(i, i == self.tab, &self.tab_label(i))).collect();
            host::page_edit(page, "tabs", &PageChange::Html(tabs));
        }
        host::page_edit(page, "journal", &class(!hovered, "idle"));
        // A locked window can't be resized, so it must not advertise a grip.
        host::page_edit(page, "journal", &class(self.locked, "locked"));
        host::page_edit(page, "opt", &class(self.opt_open, "on"));
        host::page_edit(page, "lock", &PageChange::Text(if self.locked { "LOCK" } else { "MOVE" }.into()));
    }

    fn paint_lines(&mut self, page: u64, show_all: bool) {
        // Newest last: walk the tail of the list that passes the tab + freshness
        // filter, newest first, and write them oldest first.
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

        // The RAW server hue as `hue(N)`: the host paints the line exactly as the
        // built-in log does — baked colour, legacy black border. Converting the hue
        // to RGB here would be both flatter (no border) and one table entry off.
        // One markup write per change of the list, not per line and not per frame.
        let html: String = picked.iter().rev().map(|l| line_html(&l.font, self.hue_of(l), &l.text)).collect();
        host::page_edit(page, "lines", &PageChange::Html(html));
        self.next_expire = next_expire;
    }

    fn click(&mut self, click: &str, cmds: &mut Commands) {
        match click.split_once(':') {
            Some(("tab", i)) => {
                let Some(i) = i.parse().ok().filter(|&i| i < self.tab_count() && i != self.tab) else { return };
                self.tab = i;
                self.scroll = 0; // a different channel, a different history
                self.dirty = true;
                self.save();
            }
            None if click == "lock" => {
                self.locked = !self.locked;
                self.apply_lock(cmds);
                self.save();
            }
            None if click == "opt" => self.opt_open = !self.opt_open,
            _ => {}
        }
    }

    // ---- options window ----------------------------------------------

    fn tick_opt(&mut self, cmds: &mut Commands) {
        // Closed by OPT, or by its own X (then it is already gone).
        if let Some(opt) = self.opt.filter(|&o| !self.opt_open || !host::page_is_open(o)) {
            host::page_close(opt);
            self.opt = None;
            self.opt_open = false;
        }
        if !self.opt_open {
            return;
        }
        let Some(opt) = self.opt else {
            // Just above the journal. Separate window: it never fades.
            let h = OPT_H + OPT_ROW_H * (self.tabs.len() + self.rules.len()) as f32;
            let Some(opt) = host::page_spawn(OPT_PAGE, OPT_HTML, OPT_CSS, self.x as i32, (self.y - h - 4.0).max(0.0) as i32) else {
                self.opt_open = false;
                return;
            };
            cmds.entity(Entity::from_bits(opt)).insert((GlobalZIndex { value: OPT_Z }, UiNoRightClickClose {}, UiName { value: OPT.into() }));
            self.opt = Some(opt);
            (self.opt_rows, self.opt_dirty) = (0, true);
            return;
        };

        // A reload made new inputs holding what the last fill wrote: fill again
        // before reading any, or their stale values would come back as edits.
        let rows = host::page_element(opt, "tab-rows").unwrap_or(0);
        if rows != self.opt_rows {
            (self.opt_rows, self.opt_dirty) = (rows, true);
        } else if rows != 0 {
            self.opt_poll(opt);
        }
        for click in host::page_clicks(opt) {
            self.opt_click(&click);
        }
        if self.opt_dirty {
            self.opt_dirty = false;
            self.opt_seen.clear();
            self.fill_opt(opt);
        }
    }

    // One .row per custom tab and per rule, holding the current values.
    fn fill_opt(&self, opt: u64) {
        let tabs: String = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let cells = [
                    input_html('t', i, "name", &t.name),
                    button_html("type", "tab-type", i, &filters::type_name(t.filter.ty)),
                    input_html('t', i, "text", &t.filter.text),
                    input_html('t', i, "hue", &filters::hue_text(t.filter.hue)),
                    button_html("del", "tab-del", i, "x"),
                ];
                format!(r#"<div class="row">{}</div>"#, cells.concat())
            })
            .collect();
        let rules: String = self
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let cells = [
                    button_html("type", "rule-type", i, &filters::type_name(r.filter.ty)),
                    input_html('r', i, "text", &r.filter.text),
                    input_html('r', i, "hue", &filters::hue_text(r.filter.hue)),
                    input_html('r', i, "color", &filters::hue_text(r.color)),
                    button_html("hide", "rule-hide", i, if r.hide { "HIDE" } else { "show" }),
                    button_html("del", "rule-del", i, "x"),
                ];
                format!(r#"<div class="row">{}</div>"#, cells.concat())
            })
            .collect();
        host::page_edit(opt, "tab-rows", &PageChange::Html(tabs));
        host::page_edit(opt, "rule-rows", &PageChange::Html(rules));
    }

    // Typing in an options field: an input whose value moved since the last read.
    fn opt_poll(&mut self, opt: u64) {
        let tabs = (0..self.tabs.len()).flat_map(|i| TAB_FIELDS.map(|f| ('t', i, f)));
        let rules = (0..self.rules.len()).flat_map(|i| RULE_FIELDS.map(|f| ('r', i, f)));
        let fields: Vec<(char, usize, &str)> = tabs.chain(rules).collect();
        let values = fields.iter().map(|(kind, i, f)| host::page_value(opt, &format!("{kind}{i}-{f}")).unwrap_or_default()).collect();
        let seen = std::mem::replace(&mut self.opt_seen, values);
        if seen.len() != fields.len() {
            return; // the first read after a fill
        }
        let mut changed = false;
        for (n, &(kind, i, field)) in fields.iter().enumerate() {
            if self.opt_seen[n] == seen[n] {
                continue;
            }
            let value = filters::clean(&self.opt_seen[n]);
            match (kind, field) {
                ('t', "name") => {
                    self.tabs[i].name = value;
                    self.tabs_dirty = true;
                }
                ('t', "text") => self.tabs[i].filter.text = value,
                ('t', _) => self.tabs[i].filter.hue = filters::parse_hue(&value),
                (_, "text") => self.rules[i].filter.text = value,
                (_, "hue") => self.rules[i].filter.hue = filters::parse_hue(&value),
                _ => self.rules[i].color = filters::parse_hue(&value),
            }
            changed = true;
        }
        if changed {
            self.filters_changed();
        }
    }

    fn opt_click(&mut self, click: &str) {
        let (verb, i) = click.split_once(':').map_or((click, 0), |(v, i)| (v, i.parse().unwrap_or(usize::MAX)));
        match verb {
            "add-tab" if self.tabs.len() < MAX_CUSTOM_TABS => {
                self.tabs.push(CustomTab::default());
                self.tabs_dirty = true;
            }
            "add-rule" if self.rules.len() < MAX_RULES => self.rules.push(Rule::default()),
            "tab-type" if i < self.tabs.len() => {
                let fl = &mut self.tabs[i].filter;
                fl.ty = filters::next_type(fl.ty);
            }
            "tab-del" if i < self.tabs.len() => {
                // Keep the selection on the same tab when an earlier one goes away.
                let idx = TAB_LABELS.len() + i;
                if self.tab == idx {
                    self.tab = 0;
                } else if self.tab > idx {
                    self.tab -= 1;
                }
                self.tabs.remove(i);
                self.tabs_dirty = true;
            }
            "rule-type" if i < self.rules.len() => {
                let fl = &mut self.rules[i].filter;
                fl.ty = filters::next_type(fl.ty);
            }
            "rule-hide" if i < self.rules.len() => self.rules[i].hide = !self.rules[i].hide,
            "rule-del" if i < self.rules.len() => {
                self.rules.remove(i);
            }
            _ => return,
        }
        self.opt_dirty = true;
        self.filters_changed();
    }

    // Filters are applied at paint, so an edit recolours/refilters the history too.
    fn filters_changed(&mut self) {
        self.dirty = true;
        self.save();
    }

    // ---- lock / persistence -----------------------------------------

    // Locked = no drag, no resize, and no window to the host: clicks reach the
    // world. Both gestures are host-owned markers, so this is just add/remove.
    fn apply_lock(&self, cmds: &mut Commands) {
        let Some(page) = self.page else { return };
        let root = Entity::from_bits(page);
        if self.locked {
            cmds.entity(root).remove::<(UiMovable, UiResizable)>();
            return;
        }
        cmds.entity(root).insert((
            UiMovable {},
            UiResizable {
                min_w: MIN_W,
                min_h: MIN_H,
                max_w: MAX_W,
                max_h: MAX_H,
                grip: GRIP_SIZE, // same square the page's #grip draws
                ..Default::default()
            },
        ));
    }

    // The whole window state as one JSON object (filters::Saved). A file that
    // doesn't parse is ignored: the window keeps its defaults.
    fn load_state(&mut self) {
        let raw = host::storage_get(Scope::Global);
        let Ok(mut s) = serde_json::from_str::<filters::Saved>(&raw) else { return };
        s.tabs.truncate(MAX_CUSTOM_TABS);
        s.rules.truncate(MAX_RULES);
        self.x = s.x;
        self.y = s.y;
        self.w = s.w.clamp(MIN_W, MAX_W);
        self.h = s.h.clamp(MIN_H, MAX_H);
        self.locked = s.locked;
        self.tabs = s.tabs;
        self.rules = s.rules;
        self.tab = s.tab.min(self.tab_count() - 1);
        self.saved_state = raw;
        self.tabs_dirty = true;
    }

    fn save(&mut self) {
        let saved = filters::Saved {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
            locked: self.locked,
            tab: self.tab,
            tabs: self.tabs.clone(),
            rules: self.rules.clone(),
        };
        let blob = serde_json::to_string_pretty(&saved).unwrap_or_default();
        if blob != self.saved_state {
            host::storage_set(Scope::Global, &blob);
            self.saved_state = blob;
        }
    }
}

// ---- markup ------------------------------------------------------------

fn class(on: bool, name: &str) -> PageChange {
    if on { PageChange::AddClass(name.into()) } else { PageChange::RemoveClass(name.into()) }
}

// Server and player text goes into markup: it must not read as tags or end an attribute.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

fn line_html(font: &str, hue: u16, text: &str) -> String {
    format!(r#"<div class="line" style="font-family: {font}; color: hue({hue})">{}</div>"#, escape(text))
}

fn tab_html(i: usize, on: bool, label: &str) -> String {
    let on = if on { " on" } else { "" };
    format!(r#"<button class="tab{on}" data-action="mod" data-arg="tab:{i}">{}</button>"#, escape(label))
}

// An options field, #t<i>-<field> or #r<i>-<field>; its class is the field.
fn input_html(kind: char, i: usize, field: &str, value: &str) -> String {
    format!(r#"<input id="{kind}{i}-{field}" class="{field}" value="{}">"#, escape(value))
}

fn button_html(class: &str, verb: &str, i: usize, label: &str) -> String {
    format!(r#"<button class="{class}" data-action="mod" data-arg="{verb}:{i}">{label}</button>"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_in_markup_is_escaped() {
        assert_eq!(escape(r#"<b a="1">&</b>"#), "&lt;b a=&quot;1&quot;&gt;&amp;&lt;/b&gt;");
        assert_eq!(
            line_html("uo-ascii-3", 946, "Bob: <hi> & bye"),
            r#"<div class="line" style="font-family: uo-ascii-3; color: hue(946)">Bob: &lt;hi&gt; &amp; bye</div>"#
        );
        assert_eq!(tab_html(5, true, "a<b"), r#"<button class="tab on" data-action="mod" data-arg="tab:5">a&lt;b</button>"#);
        assert_eq!(input_html('t', 2, "name", r#"say "x""#), r#"<input id="t2-name" class="name" value="say &quot;x&quot;">"#);
    }
}
