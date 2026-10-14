//go:build !linux

package qemu

import "os/exec"

// tieToParent has nothing to do here: only Linux can kill a child when its
// parent dies. On other systems the timeout in Run is the only guard.
func tieToParent(*exec.Cmd) {}
