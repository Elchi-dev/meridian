package cli

import (
	"crypto/rand"
	"crypto/sha256"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/authenticode"
)

// runKey makes the release key: sextant key new release.pem writes a new
// key with its certificate, and the certificate alone next to it.
func runKey(env Env, args []string) error {
	flags := flag.NewFlagSet("key", flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	name := flags.String("name", "Meridian release key", "the name in the certificate")
	rest, err := parseInterleaved(flags, args)
	if err != nil {
		return usageErrorf("%v", err)
	}
	if len(rest) != 2 || rest[0] != "new" {
		return usageErrorf("key takes new and a file: sextant key new release.pem")
	}
	path := rest[1]
	if !filepath.IsAbs(path) {
		path = filepath.Join(env.Dir, path)
	}
	certPath := strings.TrimSuffix(path, filepath.Ext(path)) + ".cer"
	if certPath == path {
		return usageErrorf("the key file needs a name that does not end in .cer, which is for its certificate")
	}
	for _, p := range []string{path, certPath} {
		if _, err := os.Stat(p); !errors.Is(err, fs.ErrNotExist) {
			return fmt.Errorf("%s is there already; sextant never replaces a key", p)
		}
	}
	signer, err := authenticode.NewSigner(*name, time.Now(), rand.Reader)
	if err != nil {
		return err
	}
	if err := writeNew(path, signer.PEM(), 0o600); err != nil {
		return err
	}
	if err := writeNew(certPath, signer.Cert.Raw, 0o644); err != nil {
		return err
	}
	fmt.Fprintf(env.Stdout, "key and certificate: %s\n", path)
	fmt.Fprintf(env.Stdout, "certificate alone:   %s\n", certPath)
	fmt.Fprintf(env.Stdout, "certificate SHA-256: %x\n", sha256.Sum256(signer.Cert.Raw))
	fmt.Fprintf(env.Stdout, "Keep %s somewhere safe and offline, then hand it to CI and remove it here:\n", filepath.Base(path))
	fmt.Fprintf(env.Stdout, "  gh secret set RELEASE_KEY < %s\n", filepath.Base(path))
	return nil
}

// writeNew writes a file that must not exist yet, so it can only be
// read by those perm allows from the start.
func writeNew(path string, data []byte, perm fs.FileMode) error {
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, perm)
	if err != nil {
		return err
	}
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}
