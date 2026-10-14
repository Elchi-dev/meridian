package cli

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func adrRepo(t *testing.T, index string) string {
	t.Helper()
	return checkout(t, map[string]string{
		"docs/adr/template.md":         "# NNNN. Title in the imperative\n\n- Status: proposed | accepted\n- Date: YYYY-MM-DD\n- Allows: outside code\n\n## Context\n",
		"docs/adr/README.md":           index,
		"docs/adr/0001-record-them.md": "# 0001. Record them\n",
		"docs/adr/0004-skip-a-few.md":  "# 0004. Skip a few\n",
		"docs/architecture/notes.md":   "# not a record\n",
	})
}

const adrIndex = "# Decision records\n\n| No. | Decision | Status |\n| --- | --- | --- |\n| [0001](0001-record-them.md) | Record them | accepted |\n| [0004](0004-skip-a-few.md) | Skip a few | accepted |\n"

func TestANewRecordGetsTheNextNumberAndARow(t *testing.T) {
	root := adrRepo(t, adrIndex)
	r := runIn(filepath.Join(root, "docs"), "adr", "new", "Start every core")
	want := filepath.Join(root, "docs", "adr", "0005-start-every-core.md")
	if r.code != ExitOK || r.stdout != want+"\n" {
		t.Fatalf("exit %d\n%s%s", r.code, r.stdout, r.stderr)
	}
	record, err := os.ReadFile(want)
	if err != nil {
		t.Fatal(err)
	}
	wantRecord := "# 0005. Start every core\n\n- Status: proposed\n- Date: " + time.Now().Format("2006-01-02") + "\n\n## Context\n"
	if string(record) != wantRecord {
		t.Errorf("got\n%s", record)
	}
	index, _ := os.ReadFile(filepath.Join(root, "docs", "adr", "README.md"))
	if !strings.HasSuffix(string(index), "| [0005](0005-start-every-core.md) | Start every core | proposed |\n") {
		t.Errorf("index\n%s", index)
	}
	if r := runIn(root, "adr", "new", "Start every core"); r.code != ExitOK || !strings.HasSuffix(r.stdout, "0006-start-every-core.md\n") {
		t.Errorf("the same title again gets the next number: %s%s", r.stdout, r.stderr)
	}
}

func TestRecordsNeedAProperTitleAndARepository(t *testing.T) {
	root := adrRepo(t, adrIndex)
	for _, args := range [][]string{{"adr"}, {"adr", "list"}, {"adr", "new"}, {"adr", "new", "two", "words"}, {"adr", "new", " "}, {"adr", "new", "One " + string(rune(0x2014)) + " two"}} {
		if r := runIn(root, args...); r.code != ExitUsage {
			t.Errorf("%q: exit %d %s", args, r.code, r.stderr)
		}
	}
	if r := runIn(root, "adr", "new", "two", "words"); !strings.Contains(r.stderr, "one title in quotes, got 2\n") {
		t.Errorf("the count of what came instead: %q", r.stderr)
	}
	if r := runIn(t.TempDir(), "adr", "new", "Somewhere else"); r.code != ExitFailure {
		t.Errorf("outside a repository: %d", r.code)
	}
	noTable := adrRepo(t, "# Decision records\n\nNone yet.\n")
	if r := runIn(noTable, "adr", "new", "First"); r.code != ExitFailure || !strings.Contains(r.stderr, "no table") {
		t.Errorf("an index without a table: %d %s", r.code, r.stderr)
	}
	if _, err := os.Stat(filepath.Join(noTable, "docs", "adr", "0005-first.md")); err == nil {
		t.Error("no record without its row")
	}
	if err := os.Remove(filepath.Join(root, "docs", "adr", "template.md")); err != nil {
		t.Fatal(err)
	}
	if r := runIn(root, "adr", "new", "Without a template"); r.code != ExitFailure {
		t.Errorf("no template: %d", r.code)
	}
	if err := os.RemoveAll(filepath.Join(root, "docs", "adr")); err != nil {
		t.Fatal(err)
	}
	if r := runIn(root, "adr", "new", "Without records"); r.code != ExitFailure {
		t.Errorf("no records folder: %d", r.code)
	}
}
