package cli

import (
	"flag"
	"fmt"
	"io"
	"os"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/lint"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/project"
)

func runLint(env Env, args []string) error {
	flags := flag.NewFlagSet("lint", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	if err := flags.Parse(args); err != nil {
		return usageErrorf("%v", err)
	}
	if flags.NArg() > 0 {
		return usageErrorf("lint takes no arguments, got %q", flags.Arg(0))
	}
	root, err := project.FindRoot(env.Dir)
	if err != nil {
		return err
	}
	findings, err := lint.Check(os.DirFS(root))
	if err != nil {
		return err
	}
	for _, f := range findings {
		fmt.Fprintln(env.Stdout, f)
	}
	if len(findings) > 0 {
		return fmt.Errorf("found %d %s", len(findings), plural(len(findings), "problem", "problems"))
	}
	return nil
}
