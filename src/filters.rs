// Custom tabs + colour/hide rules — the plain-data half of the options menu.
//
// No SDK types in here on purpose: the matching and the storage format are checked
// by the tests at the bottom with a plain host `cargo test`, no wasm and no host.

// One filter = up to three conditions, all of which must hold. An unset condition
// (ty/hue -1, text "") matches everything, so a blank filter matches every line.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub ty: i32,      // host MessageType byte, -1 = any
    pub text: String, // substring, case-insensitive, "" = any
    pub hue: i32,     // server hue, -1 = any
}

impl Default for Filter {
    fn default() -> Self {
        Filter { ty: -1, text: String::new(), hue: -1 }
    }
}

impl Filter {
    pub fn matches(&self, ty: u8, text: &str, hue: u16) -> bool {
        (self.ty < 0 || self.ty == ty as i32)
            && (self.hue < 0 || self.hue == hue as i32)
            && (self.text.is_empty() || text.to_lowercase().contains(&self.text.to_lowercase()))
    }
}

// A tab of its own: shows the lines its filter matches.
#[derive(Clone, Debug)]
pub struct CustomTab {
    pub name: String,
    pub filter: Filter,
}

impl Default for CustomTab {
    fn default() -> Self {
        CustomTab { name: "New".into(), filter: Filter::default() }
    }
}

// Recolour (color >= 0) and/or hide the lines the filter matches. First matching
// colour wins; any matching hide drops the line from every tab.
#[derive(Clone, Debug)]
pub struct Rule {
    pub filter: Filter,
    pub color: i32,
    pub hide: bool,
}

impl Default for Rule {
    fn default() -> Self {
        Rule { filter: Filter::default(), color: -1, hide: false }
    }
}

// Host Game/Data/MessageType.cs, in the order the type button cycles through.
// -1 first: "Any" is where a fresh filter starts.
pub const TYPES: &[(i32, &str)] = &[
    (-1, "Any"), (0, "Regular"), (1, "System"), (2, "Emote"), (6, "Label"),
    (7, "Focus"), (8, "Whisper"), (9, "Yell"), (10, "Spell"), (13, "Guild"),
    (14, "Alliance"), (15, "Command"), (16, "GM"), (0xFF, "Party"),
];

pub fn type_name(id: i32) -> String {
    TYPES.iter().find(|t| t.0 == id).map_or_else(|| id.to_string(), |t| t.1.to_string())
}

pub fn next_type(id: i32) -> i32 {
    match TYPES.iter().position(|t| t.0 == id) {
        Some(i) => TYPES[(i + 1) % TYPES.len()].0,
        None => -1,
    }
}

// "" -> -1 (unset). Decimal or 0x-hex, the two ways shards write hues.
// Anything else (or out of u16 range) reads as unset rather than as a hue.
pub fn parse_hue(s: &str) -> i32 {
    let s = s.trim();
    if s.is_empty() {
        return -1;
    }
    let v = match s.get(..2) {
        Some(p) if p.eq_ignore_ascii_case("0x") => i32::from_str_radix(&s[2..], 16),
        _ => s.parse::<i32>(),
    };
    match v {
        Ok(v) if (0..=0xFFFF).contains(&v) => v,
        _ => -1,
    }
}

pub fn hue_text(hue: i32) -> String {
    if hue < 0 { String::new() } else { hue.to_string() }
}

// ---- ingest ----------------------------------------------------------
// Mirrors the client: overhead speech (kind 0) is taken the way JournalPlugin
// takes it, system-log lines (kind 1) are worded the way SystemMessagePlugin
// words them.

// Overhead Command/Encoded aren't journalled, and overhead Party/Guild/Alliance
// would duplicate the copy that also arrives on the system channel.
pub fn journalled(kind: u8, ty: u8) -> bool {
    kind == 1 || matches!(ty, 0 | 1 | 2 | 3 | 6 | 7 | 8 | 9 | 10 | 16)
}

pub fn format(kind: u8, ty: u8, name: &str, text: &str) -> String {
    if kind == 0 {
        return if name.is_empty() { text.to_string() } else { format!("{name}: {text}") };
    }
    match ty {
        0xFF => format!("[Party][{name}]: {text}"),
        13 => format!("[Guild][{name}]: {text}"),
        14 => format!("[Alliance][{name}]: {text}"),
        0 | 1 if !name.is_empty() && !name.eq_ignore_ascii_case("system") => format!("{name}: {text}"),
        _ => text.to_string(),
    }
}

// Built-in tab: 1 Sys, 2 Chat, 3 Party, 4 Guild. Sys is what the client's
// bottom-left log shows (system channel only); anything spoken overhead is Chat.
pub fn tab_of(kind: u8, ty: u8) -> usize {
    match ty {
        0xFF => 3,
        13 | 14 => 4,
        _ if kind == 0 => 2,
        2 | 8 | 9 | 10 => 2,
        _ => 1,
    }
}

// ---- storage ---------------------------------------------------------
// Appended to the window's CSV blob, one entry per line, tab-separated:
//   T <name> <type> <text> <hue>
//   R <type> <text> <hue> <color> <hide>
// The old blob had no newline, so it still loads (no entries). Fields can't hold
// a tab or a newline — clean strips them when the user types one.

pub fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

pub fn serialize(tabs: &[CustomTab], rules: &[Rule]) -> String {
    let mut out = String::new();
    for t in tabs {
        let f = &t.filter;
        out += &format!("\nT\t{}\t{}\t{}\t{}", clean(&t.name), f.ty, clean(&f.text), f.hue);
    }
    for r in rules {
        let f = &r.filter;
        out += &format!("\nR\t{}\t{}\t{}\t{}\t{}", f.ty, clean(&f.text), f.hue, r.color, r.hide as u8);
    }
    out
}

// Parses every line after the first (the first is the window CSV). Malformed
// lines are skipped, not fatal: a hand-edited file should lose one entry, not all.
pub fn parse(raw: &str, tabs: &mut Vec<CustomTab>, rules: &mut Vec<Rule>, max_tabs: usize, max_rules: usize) {
    tabs.clear();
    rules.clear();
    for line in raw.split('\n').skip(1) {
        let p: Vec<&str> = line.split('\t').collect();
        if p[0] == "T" && p.len() >= 5 && tabs.len() < max_tabs {
            tabs.push(CustomTab { name: p[1].to_string(), filter: read_filter(&p, 2) });
        } else if p[0] == "R" && p.len() >= 6 && rules.len() < max_rules {
            rules.push(Rule { filter: read_filter(&p, 1), color: int(p[4]), hide: p[5] == "1" });
        }
    }
}

fn read_filter(p: &[&str], at: usize) -> Filter {
    Filter { ty: int(p[at]), text: p[at + 1].to_string(), hue: int(p[at + 2]) }
}

fn int(s: &str) -> i32 {
    s.trim().parse().unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(ty: i32, text: &str, hue: i32) -> Filter {
        Filter { ty, text: text.into(), hue }
    }

    #[test]
    fn blank_filter_matches_everything() {
        assert!(Filter::default().matches(1, "anything", 0x3B2));
    }

    #[test]
    fn text_is_contains_and_case_insensitive() {
        let x = f(-1, "you SEE", -1);
        assert!(x.matches(0, "You see: a dragon", 0));
        assert!(!x.matches(0, "you saw a dragon", 0));
    }

    #[test]
    fn every_set_condition_must_hold() {
        let x = f(13, "hail", 0x44);
        assert!(x.matches(13, "Hail, guild!", 0x44));
        assert!(!x.matches(1, "Hail, guild!", 0x44)); // type
        assert!(!x.matches(13, "Hello", 0x44)); // text
        assert!(!x.matches(13, "Hail, guild!", 0x45)); // hue
    }

    #[test]
    fn parse_hue_cases() {
        for (s, want) in [
            ("", -1), ("  ", -1), ("946", 946), ("0x3B2", 0x3B2), ("0X3b2", 0x3B2),
            ("abc", -1), ("-5", -1), ("70000", -1),
        ] {
            assert_eq!(parse_hue(s), want, "{s:?}");
        }
    }

    #[test]
    fn type_cycle_visits_every_type_and_wraps_to_any() {
        let mut seen = std::collections::HashSet::new();
        let mut id = -1;
        loop {
            assert!(seen.insert(id));
            id = next_type(id);
            if id == -1 {
                break;
            }
        }
        assert_eq!(seen.len(), TYPES.len());
        assert_eq!(type_name(0xFF), "Party");
        assert_eq!(next_type(12345), -1); // unknown restarts at Any
    }

    #[test]
    fn storage_round_trips_after_the_window_line() {
        let tabs = vec![
            CustomTab { name: "Loot".into(), filter: f(1, "gold", 0x35) },
            CustomTab { name: "tab\there".into(), filter: Filter::default() }, // a tab char gets cleaned
        ];
        let rules = vec![
            Rule { filter: f(-1, "spam", -1), hide: true, ..Default::default() },
            Rule { filter: f(0xFF, "", -1), color: 0x44, ..Default::default() },
        ];
        let raw = format!("6;320;320;150;1;2{}", serialize(&tabs, &rules));
        assert!(raw.starts_with("6;320;320;150;1;2\n"));

        let (mut t2, mut r2) = (Vec::new(), Vec::new());
        parse(&raw, &mut t2, &mut r2, 8, 10);

        assert_eq!(t2.len(), 2);
        assert_eq!(t2[0].name, "Loot");
        assert_eq!(t2[0].filter, f(1, "gold", 0x35));
        assert_eq!(t2[1].name, "tab here");
        assert_eq!(t2[1].filter, Filter::default());

        assert_eq!(r2.len(), 2);
        assert!(r2[0].hide);
        assert_eq!(r2[0].color, -1);
        assert_eq!(r2[0].filter.text, "spam");
        assert!(!r2[1].hide);
        assert_eq!(r2[1].color, 0x44);
        assert_eq!(r2[1].filter.ty, 0xFF);
    }

    #[test]
    fn old_blob_and_junk_lines_parse_without_panicking() {
        let (mut tabs, mut rules) = (vec![CustomTab::default()], vec![Rule::default()]);
        parse("6;320;320;150;1;2", &mut tabs, &mut rules, 8, 10); // pre-filters blob
        assert!(tabs.is_empty());
        assert!(rules.is_empty());

        parse("x\nT\tonly-name\n\nR\t1\nZ\ta\tb\nT\tok\t-1\t\t-1", &mut tabs, &mut rules, 8, 10);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].name, "ok");
        assert!(rules.is_empty());
    }

    #[test]
    fn parse_honours_the_caps() {
        let many: Vec<CustomTab> = (0..20).map(|i| CustomTab { name: format!("t{i}"), ..Default::default() }).collect();
        let mut tabs = Vec::new();
        parse(&format!("x{}", serialize(&many, &[])), &mut tabs, &mut Vec::new(), 8, 10);
        assert_eq!(tabs.len(), 8);
    }

    #[test]
    fn journalled_cases() {
        for (kind, ty, want) in [
            (1, 1, true),     // system channel: everything
            (1, 13, true),
            (0, 0, true),     // regular speech
            (0, 6, true),     // label
            (0, 15, false),   // command
            (0, 0xC0, false), // encoded
            (0, 0xFF, false), // party overhead: its system copy is the one kept
            (0, 13, false),
        ] {
            assert_eq!(journalled(kind, ty), want, "{kind} {ty}");
        }
    }

    #[test]
    fn format_matches_the_client() {
        assert_eq!(format(0, 0, "Bob", "hi"), "Bob: hi");
        assert_eq!(format(0, 0, "", "hi"), "hi");
        assert_eq!(format(1, 0xFF, "Bob", "hi"), "[Party][Bob]: hi");
        assert_eq!(format(1, 13, "Bob", "hi"), "[Guild][Bob]: hi");
        assert_eq!(format(1, 14, "Bob", "hi"), "[Alliance][Bob]: hi");
        assert_eq!(format(1, 0, "Bob", "hi"), "Bob: hi");
        assert_eq!(format(1, 1, "System", "hi"), "hi");
        assert_eq!(format(1, 2, "Bob", "hi"), "hi");
    }

    #[test]
    fn tab_of_cases() {
        for (kind, ty, tab) in [
            (0, 0, 2),    // overhead is never Sys: speech,
            (0, 1, 2),    //   overhead "system" text,
            (0, 6, 2),    //   labels
            (1, 0, 1),    // system channel -> Sys
            (1, 1, 1),
            (1, 9, 2),    // off-screen yell routed to the log -> Chat
            (1, 0xFF, 3),
            (1, 14, 4),
        ] {
            assert_eq!(tab_of(kind, ty), tab, "{kind} {ty}");
        }
    }
}
