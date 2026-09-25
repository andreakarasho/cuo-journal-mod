using CuoJournal;
using Xunit;

namespace CuoJournal.Tests;

public class FiltersTests
{
    [Fact]
    public void Blank_filter_matches_everything()
    {
        Assert.True(new Filter().Matches(1, "anything", 0x3B2));
    }

    [Fact]
    public void Text_is_contains_and_case_insensitive()
    {
        var f = new Filter { Text = "you SEE" };
        Assert.True(f.Matches(0, "You see: a dragon", 0));
        Assert.False(f.Matches(0, "you saw a dragon", 0));
    }

    [Fact]
    public void Every_set_condition_must_hold()
    {
        var f = new Filter { Type = 13, Text = "hail", Hue = 0x44 };
        Assert.True(f.Matches(13, "Hail, guild!", 0x44));
        Assert.False(f.Matches(1, "Hail, guild!", 0x44));   // type
        Assert.False(f.Matches(13, "Hello", 0x44));         // text
        Assert.False(f.Matches(13, "Hail, guild!", 0x45));  // hue
    }

    [Theory]
    [InlineData("", -1)]
    [InlineData("  ", -1)]
    [InlineData("946", 946)]
    [InlineData("0x3B2", 0x3B2)]
    [InlineData("0X3b2", 0x3B2)]
    [InlineData("abc", -1)]
    [InlineData("-5", -1)]
    [InlineData("70000", -1)]
    public void ParseHue(string s, int expected) => Assert.Equal(expected, Filters.ParseHue(s));

    [Fact]
    public void Type_cycle_visits_every_type_and_wraps_to_Any()
    {
        var seen = new HashSet<int>();
        var id = -1;
        do { Assert.True(seen.Add(id)); id = Filters.NextType(id); } while (id != -1);
        Assert.Equal(Filters.Types.Length, seen.Count);
        Assert.Equal("Party", Filters.TypeName(0xFF));
        Assert.Equal(-1, Filters.NextType(12345));   // unknown restarts at Any
    }

    [Fact]
    public void Storage_round_trips_after_the_window_line()
    {
        var tabs = new List<CustomTab>
        {
            new() { Name = "Loot", Filter = new Filter { Type = 1, Text = "gold", Hue = 0x35 } },
            new() { Name = "tab\there", Filter = new Filter() },   // a tab char gets cleaned
        };
        var rules = new List<Rule>
        {
            new() { Filter = new Filter { Text = "spam" }, Hide = true },
            new() { Filter = new Filter { Type = 0xFF }, Color = 0x44 },
        };
        var raw = "6;320;320;150;1;2" + Filters.Serialize(tabs, rules);
        Assert.StartsWith("6;320;320;150;1;2\n", raw);

        var t2 = new List<CustomTab>();
        var r2 = new List<Rule>();
        Filters.Parse(raw, t2, r2, 8, 10);

        Assert.Equal(2, t2.Count);
        Assert.Equal("Loot", t2[0].Name);
        Assert.Equal((1, "gold", 0x35), (t2[0].Filter.Type, t2[0].Filter.Text, t2[0].Filter.Hue));
        Assert.Equal("tab here", t2[1].Name);
        Assert.Equal((-1, "", -1), (t2[1].Filter.Type, t2[1].Filter.Text, t2[1].Filter.Hue));

        Assert.Equal(2, r2.Count);
        Assert.True(r2[0].Hide);
        Assert.Equal(-1, r2[0].Color);
        Assert.Equal("spam", r2[0].Filter.Text);
        Assert.False(r2[1].Hide);
        Assert.Equal(0x44, r2[1].Color);
        Assert.Equal(0xFF, r2[1].Filter.Type);
    }

    [Fact]
    public void Old_blob_and_junk_lines_parse_without_throwing()
    {
        var tabs = new List<CustomTab> { new() };
        var rules = new List<Rule> { new() };
        Filters.Parse("6;320;320;150;1;2", tabs, rules, 8, 10);   // pre-filters blob
        Assert.Empty(tabs);
        Assert.Empty(rules);

        Filters.Parse("x\nT\tonly-name\n\nR\t1\nZ\ta\tb\nT\tok\t-1\t\t-1", tabs, rules, 8, 10);
        Assert.Single(tabs);
        Assert.Equal("ok", tabs[0].Name);
        Assert.Empty(rules);
    }

    [Fact]
    public void Parse_honours_the_caps()
    {
        var many = Enumerable.Range(0, 20).Select(i => new CustomTab { Name = $"t{i}" }).ToList();
        var tabs = new List<CustomTab>();
        Filters.Parse("x" + Filters.Serialize(many, new List<Rule>()), tabs, new List<Rule>(), 8, 10);
        Assert.Equal(8, tabs.Count);
    }
}

public class IngestTests
{
    [Theory]
    [InlineData(1, 1, true)]      // system channel: everything
    [InlineData(1, 13, true)]
    [InlineData(0, 0, true)]      // regular speech
    [InlineData(0, 6, true)]      // label
    [InlineData(0, 15, false)]    // command
    [InlineData(0, 0xC0, false)]  // encoded
    [InlineData(0, 0xFF, false)]  // party overhead: its system copy is the one kept
    [InlineData(0, 13, false)]
    public void Journalled(byte kind, byte type, bool expected)
        => Assert.Equal(expected, Filters.Journalled(kind, type));

    [Fact]
    public void Format_matches_the_client()
    {
        Assert.Equal("Bob: hi", Filters.Format(0, 0, "Bob", "hi"));
        Assert.Equal("hi", Filters.Format(0, 0, "", "hi"));
        Assert.Equal("[Party][Bob]: hi", Filters.Format(1, 0xFF, "Bob", "hi"));
        Assert.Equal("[Guild][Bob]: hi", Filters.Format(1, 13, "Bob", "hi"));
        Assert.Equal("[Alliance][Bob]: hi", Filters.Format(1, 14, "Bob", "hi"));
        Assert.Equal("Bob: hi", Filters.Format(1, 0, "Bob", "hi"));
        Assert.Equal("hi", Filters.Format(1, 1, "System", "hi"));
        Assert.Equal("hi", Filters.Format(1, 2, "Bob", "hi"));
    }

    [Theory]
    [InlineData(0, 0, 2)]      // overhead is never Sys: speech,
    [InlineData(0, 1, 2)]      //   overhead "system" text,
    [InlineData(0, 6, 2)]      //   labels
    [InlineData(1, 0, 1)]      // system channel -> Sys
    [InlineData(1, 1, 1)]
    [InlineData(1, 9, 2)]      // off-screen yell routed to the log -> Chat
    [InlineData(1, 0xFF, 3)]
    [InlineData(1, 14, 4)]
    public void TabOf(byte kind, byte type, int tab)
        => Assert.Equal(tab, Filters.TabOf(kind, type));
}
