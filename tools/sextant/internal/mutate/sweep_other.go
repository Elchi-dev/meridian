//go:build !linux

package mutate

// sweep does nothing here: only Linux shows the environment of other
// processes without extra tools. Processes a run started in groups of
// their own can outlive it on other systems.
func sweep(dir string) {}

// held is never known here, so no run is stopped for the memory it holds.
func held(dir string) int64 { return 0 }
