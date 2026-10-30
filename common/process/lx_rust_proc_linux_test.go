//go:build linux && cgo && with_lx_rust_proc && !with_lx_rust_proc_rust2

package process

import (
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"testing"
)

func TestLXRustProcessPathScan(t *testing.T) {
	inode := processPathSockets(t, 1)[0]
	executable, err := os.Readlink("/proc/self/exe")
	if err != nil {
		t.Fatal(err)
	}
	for _, uid := range []uint32{uint32(os.Getuid()), processPathsAllUsers} {
		paths, err := scanProcessPathsRust(pathProc, uid)
		if err != nil {
			t.Fatal(err)
		}
		if !slices.Contains(paths[inode], executable) {
			t.Fatal("Rust scanner lost live socket owner")
		}
	}
	root := t.TempDir()
	for _, pid := range []string{"10", "2"} {
		dir := filepath.Join(root, pid)
		if err := os.MkdirAll(filepath.Join(dir, "fd"), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink("/app/"+pid+"\xff", filepath.Join(dir, "exe")); err != nil {
			t.Fatal(err)
		}
		for _, fd := range []string{"1", "2"} {
			if err := os.Symlink("socket:[42]", filepath.Join(dir, "fd", fd)); err != nil {
				t.Fatal(err)
			}
		}
	}
	paths, err := scanProcessPathsRust(root, processPathsAllUsers)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(paths, map[uint32][]string{42: {"/app/10\xff", "/app/2\xff"}}) {
		t.Fatal("Rust fixture order/dedup/raw bytes changed")
	}
	if _, err := scanProcessPathsRust(filepath.Join(root, "missing"), processPathsAllUsers); !os.IsNotExist(err) {
		t.Fatal("Rust scan lost ENOENT")
	}
}
