package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// fakeSecurity is a stand-in for macOS's security(1): it keeps one password
// in a file and logs every argv, so a test can check the key never was one.
const fakeSecurity = `#!/bin/sh
echo "$*" >> "$FAKE_KEYCHAIN.argv"
case "$1" in
-i)
	read -r cmd
	set -- $cmd
	while [ $# -gt 0 ]; do [ "$1" = "-w" ] && { printf %s "$2" > "$FAKE_KEYCHAIN"; exit 0; }; shift; done
	exit 0 ;;
find-generic-password)
	[ -f "$FAKE_KEYCHAIN" ] || exit 44
	cat "$FAKE_KEYCHAIN"; echo ;;
delete-generic-password)
	[ -f "$FAKE_KEYCHAIN" ] || exit 44
	rm "$FAKE_KEYCHAIN" ;;
esac
`

// These set package variables and the environment: not parallel.
func TestKeychainStoreReadForget(t *testing.T) {
	dir := t.TempDir()
	tool := filepath.Join(dir, "security")
	must(t, os.WriteFile(tool, []byte(fakeSecurity), 0o755))
	store := filepath.Join(dir, "keychain")
	t.Setenv("FAKE_KEYCHAIN", store)
	t.Setenv("LINEARCTL_HOME", "")
	t.Setenv("LINEAR_API_KEY", "")
	oldHas, oldBin := hasKeychain, securityBin
	hasKeychain, securityBin = true, tool
	defer func() { hasKeychain, securityBin = oldHas, oldBin }()

	if !useKeychain() {
		t.Fatal("not using the keychain")
	}
	if k, src, err := storedKey(); k != "" || src != "keychain" || err != nil {
		t.Fatalf("empty: %q %q %v", k, src, err)
	}
	if _, _, err := apiKey(); err != nil {
		t.Fatal(err)
	}
	must(t, storeKey(testKey))
	if k, _, err := storedKey(); k != testKey || err != nil {
		t.Fatalf("read back %q %v", k, err)
	}
	if removed, err := forgetKey(); !removed || err != nil {
		t.Errorf("forget: %v %v", removed, err)
	}
	if removed, err := forgetKey(); removed || err != nil {
		t.Errorf("forget again: %v %v", removed, err)
	}
	argv, _ := os.ReadFile(store + ".argv")
	if strings.Contains(string(argv), testKey) || !strings.Contains(string(argv), "find-generic-password -s linearctl -a api-key -w") {
		t.Errorf("argv log:\n%s", argv)
	}
	if err := storeKey("has space in it 00000000"); err == nil {
		t.Error("stored a key security -i would split")
	}
	// LINEARCTL_HOME takes it off the keychain.
	t.Setenv("LINEARCTL_HOME", dir)
	if useKeychain() {
		t.Error("LINEARCTL_HOME still uses the keychain")
	}
}

func TestKeychainFailuresAreNamed(t *testing.T) {
	dir := t.TempDir()
	tool := filepath.Join(dir, "security")
	must(t, os.WriteFile(tool, []byte("#!/bin/sh\necho 'User interaction is not allowed.' >&2\nexit 51\n"), 0o755))
	t.Setenv("LINEARCTL_HOME", "")
	oldHas, oldBin := hasKeychain, securityBin
	hasKeychain, securityBin = true, tool
	defer func() { hasKeychain, securityBin = oldHas, oldBin }()
	if _, _, err := storedKey(); err == nil || !strings.Contains(err.Error(), "cannot read the Keychain") {
		t.Errorf("read: %v", err)
	}
	if err := storeKey(testKey); err == nil || !strings.Contains(err.Error(), "cannot write the Keychain") {
		t.Errorf("write: %v", err)
	}
	if _, err := forgetKey(); err == nil || !strings.Contains(err.Error(), "not allowed") {
		t.Errorf("forget: %v", err)
	}
}

func TestHomeFollowsXDG(t *testing.T) {
	t.Setenv("LINEARCTL_HOME", "")
	t.Setenv("HOME", "/home/ada")
	t.Setenv("XDG_CONFIG_HOME", "/xdg/config")
	t.Setenv("XDG_CACHE_HOME", "relative/cache") // XDG says to ignore it
	t.Setenv("XDG_STATE_HOME", "")
	for kind, want := range map[string]string{
		"config": "/xdg/config/linearctl", "cache": "/home/ada/.cache/linearctl", "state": "/home/ada/.local/state/linearctl",
	} {
		if got := home(kind); got != want {
			t.Errorf("%s: %q, want %q", kind, got, want)
		}
	}
	t.Setenv("LINEARCTL_HOME", "/tmp/lc")
	if got := home("config"); got != "/tmp/lc/config" {
		t.Errorf("LINEARCTL_HOME: %q", got)
	}
	t.Setenv("LINEARCTL_HOME", "")
	t.Setenv("XDG_STATE_HOME", "")
	t.Setenv("XDG_CONFIG_HOME", "")
	t.Setenv("HOME", "")
	if got := home("state"); got != "" {
		t.Errorf("no HOME: %q", got)
	}
	if _, _, err := credentialsFile(); err == nil {
		t.Error("credentials without a home")
	}
}
