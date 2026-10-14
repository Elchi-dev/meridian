package adr

import (
	"strings"
	"testing"
)

func TestTheNextNumberFollowsTheHighest(t *testing.T) {
	names := []string{"README.md", "template.md", "0001-record-decisions.md", "0012-fonts.md", "0003-zero-dependencies.md", "0099x-odd.md", "abcd-not.md", "0020"}
	if got := Next(names); got != 13 {
		t.Errorf("got %d", got)
	}
	if Next(nil) != 1 || Next([]string{"README.md"}) != 1 {
		t.Error("the first record is 1")
	}
	if Next([]string{"9999-last.md"}) != 10000 {
		t.Error("past four digits")
	}
	if Next([]string{"0007-"}) != 8 || Next([]string{"007-x.md", "00007-x.md"}) != 1 {
		t.Error("four digits and a hyphen, nothing else")
	}
}

func TestTitlesBecomeFileNames(t *testing.T) {
	cases := map[string]string{
		"Use a buddy allocator for frames": "use-a-buddy-allocator-for-frames",
		"  Write settings in .meri!  ":     "write-settings-in-meri",
		"x86_64 first, aarch64 later":      "x86-64-first-aarch64-later",
		"Grüße an Zürich":                  "gr-e-an-z-rich",
		"az09":                             "az09",
		"`{/:@[":                           "",
		"!!!":                              "",
	}
	for title, want := range cases {
		if got := Slug(title); got != want {
			t.Errorf("%q: %q", title, got)
		}
	}
	if FileName(13, "Use the APIC timer") != "0013-use-the-apic-timer.md" || FileName(7, "A") != "0007-a.md" {
		t.Error(FileName(13, "Use the APIC timer"))
	}
}

func TestTitlesAreChecked(t *testing.T) {
	if err := Check("Start every core"); err != nil {
		t.Error(err)
	}
	for r := rune(0x2010); r <= 0x2017; r++ {
		if isDash(r) != (r >= 0x2012 && r <= 0x2015) {
			t.Errorf("%U", r)
		}
	}
	for title, want := range map[string]string{
		"":                                     "needs a title",
		"   ":                                  "needs a title",
		"!?":                                   "no letters or digits",
		"One " + string(rune(0x2014)) + " two": "has a dash",
		"One " + string(rune(0x2013)) + " two": "has a dash",
	} {
		if err := Check(title); err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("%q: %v", title, err)
		}
	}
}

const template = `# NNNN. Title in the imperative

- Status: proposed | accepted | replaced by NNNN
- Date: YYYY-MM-DD
- Allows: only for records that let outside code in

## Context

# Not a heading to fill.
`

func TestTheTemplateIsFilledIn(t *testing.T) {
	got, err := Fill(template, 13, "  Start every core ", "2026-10-15")
	want := `# 0013. Start every core

- Status: proposed
- Date: 2026-10-15

## Context

# Not a heading to fill.
`
	if err != nil || got != want {
		t.Fatalf("%v\n%s", err, got)
	}
	if _, err := Fill("no heading\n", 1, "x", "d"); err == nil {
		t.Error("a template without a heading")
	}
}

func TestTheIndexGetsARowAfterTheLast(t *testing.T) {
	index := "# Decision records\n\n| No. | Decision | Status |\n| --- | --- | --- |\n| [0001](0001-a.md) | A | accepted |\n| [0002](0002-b.md) | B | accepted |\n\nMore text.\n"
	got, err := Index(index, 3, " Start every core ")
	want := "# Decision records\n\n| No. | Decision | Status |\n| --- | --- | --- |\n| [0001](0001-a.md) | A | accepted |\n| [0002](0002-b.md) | B | accepted |\n| [0003](0003-start-every-core.md) | Start every core | proposed |\n\nMore text.\n"
	if err != nil || got != want {
		t.Fatalf("%v\n%s", err, got)
	}
	if got, err := Index("| [0001](0001-a.md) | A | accepted |", 2, "B"); err != nil || got != "| [0001](0001-a.md) | A | accepted |\n| [0002](0002-b.md) | B | proposed |" {
		t.Errorf("a table on the first line: %v %q", err, got)
	}
	if _, err := Index("# Decision records\n\nNo table yet.\n", 1, "x"); err == nil {
		t.Error("an index without a table")
	}
}
