//go:build !linux

package mutate

// sweep does nothing here: only Linux shows the environment of other
// processes without extra tools. Processes a run started in groups of
// their own can outlive it on other systems.
func sweep(dir string) {}
