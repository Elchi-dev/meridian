package mutate

import (
	"bytes"
	"fmt"
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
	for _, pid := range inFolder(dir) {
		syscall.Kill(pid, syscall.SIGKILL)
	}
}

// held is the memory the processes of the run in dir hold between them, in
// bytes: their resident pages, as /proc/<pid>/statm counts them.
func held(dir string) int64 {
	var total int64
	for _, pid := range inFolder(dir) {
		statm, err := os.ReadFile(filepath.Join("/proc", strconv.Itoa(pid), "statm"))
		if err != nil {
			continue
		}
		var size, resident int64
		if _, err := fmt.Sscan(string(statm), &size, &resident); err != nil {
			continue
		}
		total += resident * int64(os.Getpagesize())
	}
	return total
}

// inFolder lists the processes whose temporary folder is dir or lies in it.
func inFolder(dir string) []int {
	want := []byte("TMPDIR=" + dir)
	var pids []int
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
				pids = append(pids, pid)
				break
			}
		}
	}
	return pids
}
