package main

import (
	"os"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/cli"
)

// Set at release time with -ldflags "-X main.version=v0.1.0".
var version = "dev"

func main() {
	// An empty dir means the working directory as well: the commands
	// resolve it again with filepath.Abs, which reports the error if it
	// still cannot be read.
	dir, _ := os.Getwd()
	os.Exit(cli.Run(os.Args[1:], cli.Env{
		Dir:     dir,
		Stdin:   os.Stdin,
		Stdout:  os.Stdout,
		Stderr:  os.Stderr,
		Version: version,
	}))
}
