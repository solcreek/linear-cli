//go:build darwin || dragonfly || freebsd || illumos || linux || netbsd || openbsd

package acceptance

import (
	"errors"
	"os"
	"syscall"
)

// tryLock takes the OS's advisory lock on f without waiting, as linearctl
// does: false when another process holds it.
func tryLock(f *os.File) (bool, error) {
	err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
	if errors.Is(err, syscall.EWOULDBLOCK) {
		return false, nil
	}
	return err == nil, err
}

func unlock(f *os.File) { _ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN) }
