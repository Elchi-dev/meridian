// Package project finds the Meridian checkout sextant is working on.
package project

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
)

// Marker is the file at the root of every Meridian checkout. It also holds
// sextant's settings.
const Marker = "sextant.meri"

// ErrNotFound means no directory from the start upward holds the marker.
var ErrNotFound = errors.New("not inside a Meridian checkout")

// FindRoot returns the nearest directory at or above start that contains
// sextant.meri.
func FindRoot(start string) (string, error) {
	dir, err := filepath.Abs(start)
	if err != nil {
		return "", err
	}
	for {
		info, err := os.Stat(filepath.Join(dir, Marker))
		if err == nil && !info.IsDir() {
			return dir, nil
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", fmt.Errorf("%w: no %s in %s or any folder above it; run sextant inside the repository", ErrNotFound, Marker, start)
		}
		dir = parent
	}
}
