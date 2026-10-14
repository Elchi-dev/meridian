package cli

import (
	"flag"
	"fmt"
	"io"
	"math"
	"os"
	"strconv"
	"strings"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/config"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/project"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/stats"
)

func runStats(env Env, args []string) error {
	flags := flag.NewFlagSet("stats", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	check := flags.Bool("check", false, "fail when a component is below the minimum ratio")
	files := flags.Bool("files", false, "list every file")
	minRatio := flags.Float64("min", 0, "minimum test lines per line of code, instead of stats.min-ratio")
	if err := flags.Parse(args); err != nil {
		return usageErrorf("%v", err)
	}
	if flags.NArg() > 0 {
		return usageErrorf("stats takes no arguments, got %q", flags.Arg(0))
	}
	root, err := project.FindRoot(env.Dir)
	if err != nil {
		return err
	}
	settings, err := config.Load(root)
	if err != nil {
		return err
	}
	if !flagSet(flags, "min") {
		*minRatio = settings.MinRatio
	}
	report, err := stats.Collect(os.DirFS(root))
	if err != nil {
		return err
	}
	writeStats(env.Stdout, report, *files)
	if !*check {
		return nil
	}
	failing := report.Failing(*minRatio)
	for _, c := range failing {
		missing := c.Missing(*minRatio)
		fmt.Fprintf(env.Stderr, "%s has %s test lines per line of code and needs %.2f: write at least %d more %s of tests.\n",
			c.Path, twoDecimals(c.Ratio()), *minRatio, missing, plural(missing, "line", "lines"))
	}
	if len(failing) > 0 {
		return fmt.Errorf("%d of %d components have fewer than %.2f test lines per line of code", len(failing), len(report.Components), *minRatio)
	}
	return nil
}

func writeStats(w io.Writer, r *stats.Report, files bool) {
	rows := [][]string{{"component", "code", "tests", "ratio", "unsafe"}}
	var code, test, unsafe int
	for _, c := range r.Components {
		rows = append(rows, []string{c.Path, strconv.Itoa(c.Code), strconv.Itoa(c.Test), ratio(c), unsafeCell(c.Kind, c.Unsafe)})
		code += c.Code
		test += c.Test
		unsafe += c.Unsafe
		if !files {
			continue
		}
		for _, f := range c.Files {
			rows = append(rows, []string{"  " + f.Path, strconv.Itoa(f.Code), strconv.Itoa(f.Test), "", unsafeCell(c.Kind, f.Unsafe)})
		}
	}
	total := &stats.Component{Code: code, Test: test}
	rows = append(rows, []string{"total", strconv.Itoa(code), strconv.Itoa(test), ratio(total), strconv.Itoa(unsafe)})
	writeTable(w, rows)
}

func ratio(c *stats.Component) string {
	r := c.Ratio()
	if math.IsInf(r, 1) {
		return "-"
	}
	return twoDecimals(r)
}

// twoDecimals cuts instead of rounding, so 1.497 shows as 1.49 and a
// component that misses the minimum never prints as if it met it.
func twoDecimals(r float64) string {
	return strconv.FormatFloat(math.Floor(r*100+1e-9)/100, 'f', 2, 64)
}

func unsafeCell(kind string, n int) string {
	if kind == "go" {
		return "-"
	}
	return strconv.Itoa(n)
}

// writeTable left-aligns the first column and right-aligns the rest, two
// spaces apart.
func writeTable(w io.Writer, rows [][]string) {
	widths := make([]int, len(rows[0]))
	for _, row := range rows {
		for i, cell := range row {
			widths[i] = max(widths[i], len(cell))
		}
	}
	for _, row := range rows {
		line := fmt.Sprintf("%-*s", widths[0], row[0])
		for i := 1; i < len(row); i++ {
			line += fmt.Sprintf("  %*s", widths[i], row[i])
		}
		fmt.Fprintln(w, strings.TrimRight(line, " "))
	}
}

func plural(n int, one, many string) string {
	if n == 1 {
		return one
	}
	return many
}

func flagSet(flags *flag.FlagSet, name string) bool {
	set := false
	flags.Visit(func(f *flag.Flag) {
		if f.Name == name {
			set = true
		}
	})
	return set
}
