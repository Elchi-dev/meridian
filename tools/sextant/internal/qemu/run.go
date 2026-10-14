package qemu

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"time"
)

// Step waits until Wait appears in the serial output, then presses Keys,
// named as the QEMU monitor names them ("ret", "down", "esc", "a").
type Step struct {
	Wait string
	Keys []string
}

type Options struct {
	Timeout time.Duration
	// Live receives the serial output as it arrives, unchanged.
	Live io.Writer
	// Script runs in order. Each step waits for text that comes after the
	// text the previous step waited for.
	Script []Step
	// StopAfterScript ends QEMU once the last step's text has appeared and
	// its keys are sent, for images that do not end by themselves.
	StopAfterScript bool
	// AnswerPress presses the key named on every "PRESS <key>" line the
	// machine writes. Test images use it to check keyboard input.
	AnswerPress bool
	// FirmwareStall, when set, gives up on a run whose firmware has not
	// printed any text after this long, and Attempts says how often to start
	// the machine before giving up for good. OVMF under KVM sometimes hangs
	// before it prints anything on some hosts; starting again gets past it.
	FirmwareStall time.Duration
	Attempts      int
}

type Result struct {
	ExitCode int
	// Serial is everything the machine wrote, cleaned of terminal escapes.
	Serial    string
	TimedOut  bool
	StepsDone int
	Stopped   bool
	// FirmwareStalled is set when the last attempt stalled before the
	// firmware printed anything. Attempts counts the machines started.
	FirmwareStalled bool
	Attempts        int
}

// Run boots the machine and waits for QEMU to end, the timeout to pass or
// the script to finish. The firmware variable store is copied first, so a
// run never changes the file it was given.
func Run(ctx context.Context, m Machine, opts Options) (Result, error) {
	if err := m.Validate(); err != nil {
		return Result{}, err
	}
	attempts := max(opts.Attempts, 1)
	for attempt := 1; ; attempt++ {
		res, err := runOnce(ctx, m, opts)
		res.Attempts = attempt
		if err != nil || !res.FirmwareStalled || attempt == attempts {
			return res, err
		}
		if opts.Live != nil {
			fmt.Fprintf(opts.Live, "\nsextant: the firmware printed nothing for %v; starting the machine again (%d of %d)\n",
				opts.FirmwareStall, attempt+1, attempts)
		}
	}
}

func runOnce(ctx context.Context, m Machine, opts Options) (Result, error) {
	work, err := os.MkdirTemp("", "sextant-qemu-")
	if err != nil {
		return Result{}, err
	}
	defer os.RemoveAll(work)
	vars := filepath.Join(work, "vars.fd")
	if err := copyFile(m.Firmware.Vars, vars); err != nil {
		return Result{}, fmt.Errorf("copy the firmware variable store: %w", err)
	}
	m.Firmware.Vars = vars

	var monitorAddr string
	if len(opts.Script) > 0 || opts.AnswerPress {
		monitorAddr, err = freeLocalAddress()
		if err != nil {
			return Result{}, err
		}
		m.Monitor = "tcp:" + monitorAddr + ",server=on,wait=off"
	}

	ctx, cancel := context.WithTimeout(ctx, opts.Timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, m.Binary, m.Args()...)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return Result{}, err
	}
	if err := cmd.Start(); err != nil {
		return Result{}, fmt.Errorf("start %s: %w", m.Binary, err)
	}

	watcher := &scriptWatcher{steps: opts.Script, address: monitorAddr, stop: cancel, stopAfter: opts.StopAfterScript, answer: opts.AnswerPress}
	var stalled atomic.Bool
	if opts.FirmwareStall > 0 {
		timer := time.AfterFunc(opts.FirmwareStall, func() {
			if !watcher.sawText.Load() {
				stalled.Store(true)
				cancel()
			}
		})
		defer timer.Stop()
	}
	serial := watcher.read(stdout, opts.Live)
	waitErr := cmd.Wait()

	res := Result{Serial: Clean(serial), StepsDone: watcher.done, Stopped: watcher.stopped, FirmwareStalled: stalled.Load()}
	var exit *exec.ExitError
	switch {
	case watcher.stopped || res.FirmwareStalled:
	case errors.Is(ctx.Err(), context.DeadlineExceeded):
		res.TimedOut = true
	case errors.As(waitErr, &exit):
		res.ExitCode = exit.ExitCode()
		if res.ExitCode != ExitPass && res.ExitCode != ExitFail && strings.TrimSpace(stderr.String()) != "" {
			return res, fmt.Errorf("QEMU failed with exit code %d: %s", res.ExitCode, strings.TrimSpace(stderr.String()))
		}
	case waitErr != nil:
		return res, waitErr
	}
	if watcher.err != nil {
		return res, watcher.err
	}
	return res, nil
}

// scriptWatcher reads the serial stream and plays the script against it.
type scriptWatcher struct {
	steps     []Step
	address   string
	stop      context.CancelFunc
	stopAfter bool
	answer    bool

	done      int
	pressFrom int
	sawText   atomic.Bool
	stopped   bool
	err       error
	monitor   net.Conn
}

func (w *scriptWatcher) read(r io.Reader, live io.Writer) string {
	var all strings.Builder
	chunk := make([]byte, 4096)
	from := 0
	for {
		n, err := r.Read(chunk)
		if n > 0 {
			all.Write(chunk[:n])
			if live != nil {
				_, _ = live.Write(chunk[:n])
			}
			clean := Clean(all.String())
			if strings.TrimSpace(clean) != "" {
				w.sawText.Store(true)
			}
			from = w.advance(clean, from)
			w.answerPresses(clean)
		}
		if err != nil {
			break
		}
	}
	if w.monitor != nil {
		w.monitor.Close()
	}
	return all.String()
}

// advance runs every step whose text has appeared after position from in
// the cleaned output, and returns where the next step starts looking.
func (w *scriptWatcher) advance(clean string, from int) int {
	for w.done < len(w.steps) {
		step := w.steps[w.done]
		at := strings.Index(clean[from:], step.Wait)
		if at < 0 {
			return from
		}
		from += at + len(step.Wait)
		for _, key := range step.Keys {
			if err := w.press(key); err != nil {
				w.err = err
				w.stop()
				return from
			}
		}
		w.done++
	}
	if w.done == len(w.steps) && len(w.steps) > 0 && w.stopAfter && !w.stopped {
		w.stopped = true
		w.stop()
	}
	return from
}

// answerPresses presses the key of every complete "PRESS <key>" line it
// has not seen yet.
func (w *scriptWatcher) answerPresses(clean string) {
	for w.answer && w.err == nil {
		end := strings.IndexByte(clean[w.pressFrom:], '\n')
		if end < 0 {
			return
		}
		line := clean[w.pressFrom : w.pressFrom+end]
		w.pressFrom += end + 1
		if key, ok := strings.CutPrefix(line, "PRESS "); ok {
			if err := w.press(strings.TrimSpace(key)); err != nil {
				w.err = err
				w.stop()
			}
		}
	}
}

func (w *scriptWatcher) press(key string) error {
	if w.monitor == nil {
		var err error
		for attempt := 0; attempt < 50; attempt++ {
			w.monitor, err = net.Dial("tcp", w.address)
			if err == nil {
				break
			}
			time.Sleep(100 * time.Millisecond)
		}
		if err != nil {
			return fmt.Errorf("connect to the QEMU monitor at %s: %w", w.address, err)
		}
	}
	if _, err := fmt.Fprintf(w.monitor, "sendkey %s\n", key); err != nil {
		return fmt.Errorf("send key %q: %w", key, err)
	}
	time.Sleep(50 * time.Millisecond)
	return nil
}

// freeLocalAddress asks the system for a free TCP port on the loopback
// interface. QEMU binds it a moment later; another program taking it in
// between makes QEMU fail to start, which Run reports.
func freeLocalAddress() (string, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return "", fmt.Errorf("find a free port for the QEMU monitor: %w", err)
	}
	defer l.Close()
	return "127.0.0.1:" + strconv.Itoa(l.Addr().(*net.TCPAddr).Port), nil
}

func copyFile(from, to string) error {
	data, err := os.ReadFile(from)
	if err != nil {
		return err
	}
	return os.WriteFile(to, data, 0o644)
}
