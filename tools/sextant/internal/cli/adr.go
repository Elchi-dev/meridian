package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/adr"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/project"
)

func runADR(env Env, args []string) error {
	if len(args) == 0 || args[0] != "new" {
		return usageErrorf("adr takes new and a title: sextant adr new \"Use one thing over another\"")
	}
	if len(args) != 2 {
		return usageErrorf("adr new takes one title in quotes, got %d", len(args)-1)
	}
	title := args[1]
	if err := adr.Check(title); err != nil {
		return usageErrorf("%v", err)
	}
	root, err := project.FindRoot(env.Dir)
	if err != nil {
		return err
	}
	dir := filepath.Join(root, "docs", "adr")
	entries, err := os.ReadDir(dir)
	if err != nil {
		return err
	}
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	number := adr.Next(names)
	template, err := os.ReadFile(filepath.Join(dir, "template.md"))
	if err != nil {
		return err
	}
	record, err := adr.Fill(string(template), number, title, time.Now().Format("2006-01-02"))
	if err != nil {
		return err
	}
	indexPath := filepath.Join(dir, "README.md")
	index, err := os.ReadFile(indexPath)
	if err != nil {
		return err
	}
	newIndex, err := adr.Index(string(index), number, title)
	if err != nil {
		return err
	}
	path := filepath.Join(dir, adr.FileName(number, title))
	if err := os.WriteFile(path, []byte(record), 0o644); err != nil {
		return err
	}
	if err := os.WriteFile(indexPath, []byte(newIndex), 0o644); err != nil {
		return err
	}
	fmt.Fprintln(env.Stdout, path)
	return nil
}
