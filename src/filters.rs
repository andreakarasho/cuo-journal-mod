// Custom tabs + colour/hide rules — the plain-data half of the options menu.
//
use serde::{Deserialize, Serialize};

// No SDK types in here on purpose: the matching and the storage format are checked
// by the tests at the bottom with a plain host `cargo test`, no wasm and no host.

// One filter = up to three conditions, all of which must hold. An unset condition
// (ty/hue -1, text "") matches everything, so a blank filter matches every line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
// The whole window state as one JSON object (Data/Mods/journal/storage.json).
// Missing tabs/rules or filter fields fall back to their defaults; anything else
// malformed and the file is ignored (the window keeps its defaults).

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Saved {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub locked: bool,
    pub tab: usize,
    #[serde(default)]
    pub tabs: Vec<CustomTab>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

// Option fields are single-line: a typed tab/newline becomes a space.
pub fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
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
    fn storage_round_trips_as_json() {
        let saved = Saved {
            x: 6.0, y: 320.0, w: 320.0, h: 150.5, locked: true, tab: 2,
            tabs: vec![CustomTab { name: "Loot \"q\"".into(), filter: f(1, "gold", 0x35) }],
            rules: vec![Rule { filter: f(-1, "spam", -1), hide: true, ..Default::default() }],
        };
        let raw = serde_json::to_string_pretty(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&raw).unwrap(), saved);
    }

    #[test]
    fn storage_defaults_missing_fields_and_rejects_junk() {
        let s: Saved = serde_json::from_str(
            r#"{"x":1,"y":2,"w":300,"h":100,"locked":false,"tab":0,"rules":[{"hide":true}]}"#,
        ).unwrap();
        assert!(s.tabs.is_empty());
        assert_eq!(s.rules, vec![Rule { hide: true, ..Default::default() }]);
        assert!(serde_json::from_str::<Saved>("7;869;452;175.6;1;0").is_err());
        assert!(serde_json::from_str::<Saved>(r#"{"x":1}"#).is_err());
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
