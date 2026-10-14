package mutate

import (
	"fmt"
	"go/build"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/srclex"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/stats"
)

// Pick returns the files worth mutating: Rust and Go sources that are not
// test files, narrowed to paths (each a file or a folder, "." for all) and,
// when changed is not nil, to the files in it.
func Pick(sources []stats.Source, paths []string, changed map[string]bool) ([]stats.Source, error) {
	var out []stats.Source
	matched := make([]bool, len(paths))
	for _, s := range sources {
		if (s.Lang != srclex.Rust && s.Lang != srclex.Go) || s.AllTest {
			continue
		}
		if changed != nil && !changed[s.Path] {
			continue
		}
		keep := len(paths) == 0
		for i, p := range paths {
			if p == "." || s.Path == p || strings.HasPrefix(s.Path, p+"/") {
				matched[i] = true
				keep = true
			}
		}
		if keep {
			out = append(out, s)
		}
	}
	for i, ok := range matched {
		if !ok {
			return nil, fmt.Errorf("%s has no Rust or Go code to mutate", paths[i])
		}
	}
	return out, nil
}

// Gather finds the mutants of every source and says which component tests
// each file. Go files this platform does not build, such as kill_windows.go
// on Linux, are skipped: no test here could ever notice their mutants.
func Gather(root string, sources []stats.Source) ([]Mutant, map[string]Target, error) {
	var mutants []Mutant
	targets := map[string]Target{}
	machine := map[string]bool{}
	for _, s := range sources {
		file := filepath.Join(root, filepath.FromSlash(s.Path))
		if s.Lang == srclex.Go {
			built, err := build.Default.MatchFile(filepath.Dir(file), filepath.Base(file))
			if err != nil {
				return nil, nil, fmt.Errorf("%s: %w", s.Path, err)
			}
			if !built {
				continue
			}
		}
		src, err := os.ReadFile(file)
		if err != nil {
			return nil, nil, err
		}
		var found []Mutant
		if s.Lang == srclex.Go {
			found, err = GoMutants(s.Path, src)
		} else {
			found, err = RustMutants(s.Path, src, s.IsTest)
		}
		if err != nil {
			return nil, nil, fmt.Errorf("%s: %w", s.Path, err)
		}
		mutants = append(mutants, found...)
		has, seen := machine[s.Component]
		if !seen && s.Kind == "rust" {
			has, err = hasMachineChecks(filepath.Join(root, filepath.FromSlash(s.Component), "Cargo.toml"))
			if err != nil {
				return nil, nil, err
			}
			machine[s.Component] = has
		}
		targets[s.Path] = Target{Component: s.Component, Kind: s.Kind, Machine: has}
	}
	return mutants, targets, nil
}

// qemuFeature is the line that gives a crate the qemu-test feature, under
// which its checks against real firmware are built.
var qemuFeature = regexp.MustCompile(`(?m)^qemu-test\s*=`)

// hasMachineChecks says whether the crate of manifest has checks that run
// in the QEMU test image.
func hasMachineChecks(manifest string) (bool, error) {
	data, err := os.ReadFile(manifest)
	if err != nil {
		return false, err
	}
	return qemuFeature.Match(data), nil
}
