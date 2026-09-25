// Custom tabs + colour/hide rules — the plain-data half of the options menu.
//
// No SDK types in here on purpose: tests/ compiles this file on its own against
// plain net10.0, so the matching and the storage format are checked without a host.

using System.Globalization;
using System.Text;

namespace CuoJournal;

// One filter = up to three conditions, all of which must hold. An unset condition
// (Type/Hue -1, Text "") matches everything, so a blank filter matches every line.
public sealed class Filter
{
    public int Type = -1;        // host MessageType byte, -1 = any
    public string Text = "";     // substring, case-insensitive, "" = any
    public int Hue = -1;         // server hue, -1 = any

    public bool Matches(byte type, string text, ushort hue)
        => (Type < 0 || Type == type)
        && (Hue < 0 || Hue == hue)
        && (Text.Length == 0 || text.Contains(Text, StringComparison.OrdinalIgnoreCase));
}

// A tab of its own: shows the lines its filter matches.
public sealed class CustomTab
{
    public string Name = "New";
    public Filter Filter = new();
}

// Recolour (Color >= 0) and/or hide the lines the filter matches. First matching
// colour wins; any matching Hide drops the line from every tab.
public sealed class Rule
{
    public Filter Filter = new();
    public int Color = -1;
    public bool Hide;
}

public static class Filters
{
    // Host Game/Data/MessageType.cs, in the order the type button cycles through.
    // -1 first: "Any" is where a fresh filter starts.
    public static readonly (int Id, string Name)[] Types =
    {
        (-1, "Any"), (0, "Regular"), (1, "System"), (2, "Emote"), (6, "Label"),
        (7, "Focus"), (8, "Whisper"), (9, "Yell"), (10, "Spell"), (13, "Guild"),
        (14, "Alliance"), (15, "Command"), (16, "GM"), (0xFF, "Party"),
    };

    public static string TypeName(int id)
    {
        foreach (var t in Types)
            if (t.Id == id) return t.Name;
        return id.ToString(CultureInfo.InvariantCulture);
    }

    public static int NextType(int id)
    {
        for (var i = 0; i < Types.Length; i++)
            if (Types[i].Id == id) return Types[(i + 1) % Types.Length].Id;
        return -1;
    }

    // "" -> -1 (unset). Decimal or 0x-hex, the two ways shards write hues.
    // Anything else (or out of ushort range) reads as unset rather than as a hue.
    public static int ParseHue(string s)
    {
        s = s.Trim();
        if (s.Length == 0) return -1;
        var ok = s.StartsWith("0x", StringComparison.OrdinalIgnoreCase)
            ? int.TryParse(s.AsSpan(2), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out var v)
            : int.TryParse(s, NumberStyles.Integer, CultureInfo.InvariantCulture, out v);
        return ok && v is >= 0 and <= 0xFFFF ? v : -1;
    }

    public static string HueText(int hue) => hue < 0 ? "" : hue.ToString(CultureInfo.InvariantCulture);

    // ---- ingest ----------------------------------------------------------
    // Mirrors the client: overhead speech (Kind 0) is taken the way JournalPlugin
    // takes it, system-log lines (Kind 1) are worded the way SystemMessagePlugin
    // words them.

    // Overhead Command/Encoded aren't journalled, and overhead Party/Guild/Alliance
    // would duplicate the copy that also arrives on the system channel.
    public static bool Journalled(byte kind, byte type)
        => kind == 1 || type is 0 or 1 or 2 or 3 or 6 or 7 or 8 or 9 or 10 or 16;

    public static string Format(byte kind, byte type, string name, string text)
    {
        if (kind == 0)
            return name.Length == 0 ? text : $"{name}: {text}";
        return type switch
        {
            0xFF => $"[Party][{name}]: {text}",
            13 => $"[Guild][{name}]: {text}",
            14 => $"[Alliance][{name}]: {text}",
            0 or 1 when name.Length > 0 && !name.Equals("system", StringComparison.OrdinalIgnoreCase)
                => $"{name}: {text}",
            _ => text,
        };
    }

    // Built-in tab: 1 Sys, 2 Chat, 3 Party, 4 Guild. Sys is what the client's
    // bottom-left log shows (system channel only); anything spoken overhead is Chat.
    public static int TabOf(byte kind, byte type) => type switch
    {
        0xFF => 3,
        13 or 14 => 4,
        _ when kind == 0 => 2,
        2 or 8 or 9 or 10 => 2,
        _ => 1,
    };

    // ---- storage ---------------------------------------------------------
    // Appended to the window's CSV blob, one entry per line, tab-separated:
    //   T <name> <type> <text> <hue>
    //   R <type> <text> <hue> <color> <hide>
    // The old blob had no newline, so it still loads (no entries). Fields can't hold
    // a tab or a newline — Clean strips them when the user types one.

    public static string Clean(string s) => s.Replace('\t', ' ').Replace('\n', ' ').Replace('\r', ' ');

    public static string Serialize(List<CustomTab> tabs, List<Rule> rules)
    {
        var sb = new StringBuilder();
        foreach (var t in tabs)
            sb.Append($"\nT\t{Clean(t.Name)}\t{t.Filter.Type}\t{Clean(t.Filter.Text)}\t{t.Filter.Hue}");
        foreach (var r in rules)
            sb.Append($"\nR\t{r.Filter.Type}\t{Clean(r.Filter.Text)}\t{r.Filter.Hue}\t{r.Color}\t{(r.Hide ? 1 : 0)}");
        return sb.ToString();
    }

    // Parses every line after the first (the first is the window CSV). Malformed
    // lines are skipped, not fatal: a hand-edited file should lose one entry, not all.
    public static void Parse(string raw, List<CustomTab> tabs, List<Rule> rules, int maxTabs, int maxRules)
    {
        tabs.Clear();
        rules.Clear();
        var lines = raw.Split('\n');
        for (var i = 1; i < lines.Length; i++)
        {
            var p = lines[i].Split('\t');
            if (p[0] == "T" && p.Length >= 5 && tabs.Count < maxTabs)
                tabs.Add(new CustomTab { Name = p[1], Filter = ReadFilter(p, 2) });
            else if (p[0] == "R" && p.Length >= 6 && rules.Count < maxRules)
                rules.Add(new Rule { Filter = ReadFilter(p, 1), Color = Int(p[4]), Hide = p[5] == "1" });
        }
    }

    static Filter ReadFilter(string[] p, int at) => new() { Type = Int(p[at]), Text = p[at + 1], Hue = Int(p[at + 2]) };

    static int Int(string s) => int.TryParse(s, NumberStyles.Integer, CultureInfo.InvariantCulture, out var v) ? v : -1;
}
