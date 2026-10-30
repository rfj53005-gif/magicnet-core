//go:build linux && cgo && with_lx_rust_proc

package process

/*
#include <stdint.h>
#include <stddef.h>
typedef struct { uint8_t *data; size_t len; int32_t error; } magicnet_scan_result;
magicnet_scan_result magicnet_proc_scan(const uint8_t *root, size_t root_len, uint32_t uid);
void magicnet_proc_free(uint8_t *data, size_t len);
#cgo linux,amd64 LDFLAGS: -L${SRCDIR}/../../experimental/lx_rust_proc/lib/linux-amd64 -lmagicnet_proc_scan -ldl -lpthread -lm
#cgo android,arm64 LDFLAGS: -L${SRCDIR}/../../experimental/lx_rust_proc/lib/android-arm64 -lmagicnet_proc_scan -ldl -lm
*/
import "C"

import (
	"encoding/binary"
	"errors"
	"runtime"
	"syscall"
	"unsafe"
)

var errInvalidRustProcessSnapshot = errors.New("invalid Rust process snapshot")

func scanProcessPathsRust(root string, uid uint32) (map[uint32][]string, error) {
	result := C.magicnet_proc_scan((*C.uint8_t)(unsafe.Pointer(unsafe.StringData(root))), C.size_t(len(root)), C.uint32_t(uid))
	runtime.KeepAlive(root)
	if result.error != 0 {
		if result.error < 0 {
			return nil, errors.New("Rust process scanner panicked")
		}
		return nil, syscall.Errno(result.error)
	}
	defer C.magicnet_proc_free(result.data, result.len)
	if result.data == nil || uint64(result.len) > 256*1024*1024 {
		return nil, errInvalidRustProcessSnapshot
	}
	// Copy every executable string before releasing the Rust-owned buffer.
	return decodeRustProcessSnapshot(unsafe.Slice((*byte)(unsafe.Pointer(result.data)), int(result.len)))
}

func decodeRustProcessSnapshot(blob []byte) (map[uint32][]string, error) {
	read := func() (uint32, bool) {
		if len(blob) < 4 {
			return 0, false
		}
		value := binary.LittleEndian.Uint32(blob)
		blob = blob[4:]
		return value, true
	}
	magic, ok := read()
	if !ok || magic != 0x3153504d {
		return nil, errInvalidRustProcessSnapshot
	}
	pathCount, ok := read()
	if !ok {
		return nil, errInvalidRustProcessSnapshot
	}
	entryCount, ok := read()
	if !ok || uint64(pathCount)*4+uint64(entryCount)*8 > uint64(len(blob)) {
		return nil, errInvalidRustProcessSnapshot
	}
	executables := make([]string, int(pathCount))
	for i := range executables {
		size, ok := read()
		if !ok || uint64(size) > uint64(len(blob)) {
			return nil, errInvalidRustProcessSnapshot
		}
		executables[i] = string(blob[:size])
		blob = blob[size:]
	}
	entries := make(map[uint32][]string, int(entryCount))
	for range entryCount {
		inode, ok := read()
		if !ok {
			return nil, errInvalidRustProcessSnapshot
		}
		count, ok := read()
		if !ok || uint64(count)*4 > uint64(len(blob)) {
			return nil, errInvalidRustProcessSnapshot
		}
		if _, exists := entries[inode]; exists {
			return nil, errInvalidRustProcessSnapshot
		}
		owners := make([]string, int(count))
		for i := range owners {
			id, ok := read()
			if !ok || id >= pathCount {
				return nil, errInvalidRustProcessSnapshot
			}
			owners[i] = executables[id]
		}
		entries[inode] = owners
	}
	if len(blob) != 0 {
		return nil, errInvalidRustProcessSnapshot
	}
	return entries, nil
}

func buildProcessPathsSelected(uid uint32) (map[uint32][]string, error) {
	return scanProcessPathsRust(pathProc, uid)
}
