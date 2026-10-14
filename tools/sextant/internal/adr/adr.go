// Package adr starts new decision records in docs/adr: the next number,
// a file name from the title, the template filled in, and a row in the
// index.
package adr

import (
	"errors"
	"fmt"
	"strconv"
	"strings"
)

// Next is the number after the highest record among the file names, or
// 1 when there is none. Only names that start with four digits and a
// hyphen count.
func Next(names []string) int {
	highest := 0
	for _, name := range names {
		prefix, _, found := strings.Cut(name, "-")
		if n, err := strconv.Atoi(prefix); found && len(prefix) == 4 && err == nil {
			highest = max(highest, n)
		}
	}
	return highest + 1
}

// Slug turns a title into a file name: lower case letters and digits,
// with single hyphens for everything between them.
func Slug(title string) string {
	var b strings.Builder
	var gap bool
	for _, r := range strings.ToLower(title) {
		if 'a' <= r && r <= 'z' || '0' <= r && r <= '9' {
			if gap && b.Len() > 0 {
				b.WriteByte('-')
			}
			b.WriteRune(r)
			gap = false
		} else {
			gap = true
		}
	}
	return b.String()
}

// FileName is the record's file name, such as 0013-use-a-buddy-allocator.md.
func FileName(number int, title string) string {
	return fmt.Sprintf("%04d-%s.md", number, Slug(title))
}

// Check says why title cannot be a record's title, if it cannot.
func Check(title string) error {
	switch {
	case strings.TrimSpace(title) == "":
		return errors.New("a record needs a title")
	case Slug(title) == "":
		return fmt.Errorf("the title %q has no letters or digits to name the file after", title)
	case strings.ContainsFunc(title, isDash):
		return fmt.Errorf("the title %q has a dash, which the project does not use; take a comma or a colon", title)
	}
	return nil
}

// isDash says whether r is one of the dashes the project does not use:
// the figure dash, the en and em dashes and the horizontal bar.
func isDash(r rune) bool {
	return 0x2012 <= r && r <= 0x2015
}

// Fill is the template with the record's number, title and date, its
// status proposed, and without the line that lets outside code in, which
// a record adds only when it needs it.
func Fill(template string, number int, title, date string) (string, error) {
	var out []string
	heading := false
	for _, line := range strings.Split(template, "\n") {
		switch {
		case strings.HasPrefix(line, "# ") && !heading:
			line, heading = fmt.Sprintf("# %04d. %s", number, strings.TrimSpace(title)), true
		case strings.HasPrefix(line, "- Status:"):
			line = "- Status: proposed"
		case strings.HasPrefix(line, "- Date:"):
			line = "- Date: " + date
		case strings.HasPrefix(line, "- Allows:"):
			continue
		}
		out = append(out, line)
	}
	if !heading {
		return "", errors.New("the template has no heading to put the title in")
	}
	return strings.Join(out, "\n"), nil
}

// Index is the index with a row for the record after the table's last
// row.
func Index(index string, number int, title string) (string, error) {
	lines := strings.Split(index, "\n")
	last := -1
	for i, line := range lines {
		if strings.HasPrefix(line, "| [") {
			last = i
		}
	}
	if last < 0 {
		return "", errors.New("the index has no table of records to add to")
	}
	row := fmt.Sprintf("| [%04d](%s) | %s | proposed |", number, FileName(number, title), strings.TrimSpace(title))
	lines = append(lines[:last+1], append([]string{row}, lines[last+1:]...)...)
	return strings.Join(lines, "\n"), nil
}
