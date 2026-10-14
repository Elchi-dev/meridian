package mutate

import (
	"bytes"
	"os"
	"path/filepath"
	"strconv"
	"syscall"
)

// sweep kills every process whose temporary folder is dir or lies in it.
// Each run gets a TMPDIR of its own and children inherit it, so this finds
// what a run left behind in process groups of their own, which reapTree
// cannot reach. sextant testing itself is the case that needs it: its tests
// start runners whose tests run in groups of their own, and a mutant that
// makes such a test hang leaves those behind when the outer run is killed.
func sweep(dir string) {
	want := []byte("TMPDIR=" + dir)
	procs, _ := os.ReadDir("/proc")
	for _, p := range procs {
		pid, err := strconv.Atoi(p.Name())
		if err != nil {
			continue
		}
		env, err := os.ReadFile(filepath.Join("/proc", p.Name(), "environ"))
		if err != nil {
			continue
		}
		for _, kv := range bytes.Split(env, []byte{0}) {
			if bytes.HasPrefix(kv, want) && (len(kv) == len(want) || kv[len(want)] == '/') {
				syscall.Kill(pid, syscall.SIGKILL)
			}
		}
	}
}
