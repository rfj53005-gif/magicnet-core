//go:build linux && cgo && with_lx_rust_proc && with_lx_rust_proc_rust2

package process

/*
#include <stdint.h>
#include <stddef.h>
typedef struct { uint8_t *data; size_t len; int32_t error; } magicnet_scan_result;
magicnet_scan_result magicnet_proc_scan(const uint8_t *root, size_t root_len, uint32_t uid);
void magicnet_proc_free(uint8_t *data, size_t len);
uint32_t magicnet_proc_scanner_variant(void);
#cgo linux,amd64 LDFLAGS: -L${SRCDIR}/../../experimental/lx_rust_proc/lib/linux-amd64-rust2 -lmagicnet_proc_scan -ldl -lpthread -lm
#cgo android,arm64 LDFLAGS: -L${SRCDIR}/../../experimental/lx_rust_proc/lib/android-arm64-rust2 -lmagicnet_proc_scan -ldl -lm
*/
import "C"

import (
	"encoding/binary"
	"errors"
	"fmt"
	"runtime"
	"sync"
	"syscall"
	"unsafe"
)

const (
	// rustProcessSnapshotV2Magic is "MPS1" in little endian. Rust2 keeps the
	// Rust1 wire format; only the scanner implementation changed.
	rustProcessSnapshotV2Magic uint32 = 0x3153504d

	// rustProcessSnapshotV2MaxBytes mirrors the Rust MAX_BLOB limit.
	rustProcessSnapshotV2MaxBytes = 256 * 1024 * 1024

	// rustProcessOwnerChunkSize is the number of string headers allocated per
	// chunk for process path owner lists.
	rustProcessOwnerChunkSize = 32
)

var (
	errInvalidRustProcessSnapshotV2 = errors.New("invalid Rust process snapshot")
	errRustProcessScannerVariant    = errors.New("Rust process scanner variant mismatch")

	rust2ScannerVariantOnce sync.Once
	rust2ScannerVariantErr  error
)

// rust2ScannerVariant reports the variant encoded in the linked static library.
func rust2ScannerVariant() uint32 {
	return uint32(C.magicnet_proc_scanner_variant())
}

// ensureRust2ScannerVariant rejects a Rust1 static library linked under the
// Rust2 build tag instead of silently decoding it with the wrong expectations.
func ensureRust2ScannerVariant() error {
	rust2ScannerVariantOnce.Do(func() {
		if variant := rust2ScannerVariant(); variant != 2 {
			rust2ScannerVariantErr = fmt.Errorf("%w: static library reports variant %d, want 2", errRustProcessScannerVariant, variant)
		}
	})
	return rust2ScannerVariantErr
}

func scanProcessPathsRust2(root string, uid uint32) (map[uint32][]string, error) {
	if err := ensureRust2ScannerVariant(); err != nil {
		return nil, err
	}
	result := C.magicnet_proc_scan((*C.uint8_t)(unsafe.Pointer(unsafe.StringData(root))), C.size_t(len(root)), C.uint32_t(uid))
	runtime.KeepAlive(root)
	if result.error != 0 {
		if result.error < 0 {
			return nil, errors.New("Rust process scanner panicked")
		}
		return nil, syscall.Errno(result.error)
	}
	defer C.magicnet_proc_free(result.data, result.len)
	if result.data == nil || uint64(result.len) > rustProcessSnapshotV2MaxBytes {
		return nil, errInvalidRustProcessSnapshotV2
	}
	// Every returned string is copied before the deferred Rust free runs.
	return decodeRustProcessSnapshotV2(unsafe.Slice((*byte)(unsafe.Pointer(result.data)), int(result.len)))
}

// decodeRustProcessSnapshotV2 validates and decodes one MPS1 blob in a single
// pass. Validation is as strict as the Rust1 decoder: magic, minimum-length
// preflight against absurd counts, byte-exact truncation, unique inodes, path
// IDs below pathCount and no trailing bytes.
//
// Owner lists are carved out of 32-entry chunks. A returned slice always has
// cap == len, so appending to one entry cannot overwrite another entry's data.
// An inode with more than 32 holders gets its own exact allocation.
func decodeRustProcessSnapshotV2(blob []byte) (map[uint32][]string, error) {
	read := func() (uint32, bool) {
		if len(blob) < 4 {
			return 0, false
		}
		value := binary.LittleEndian.Uint32(blob)
		blob = blob[4:]
		return value, true
	}
	magic, ok := read()
	if !ok || magic != rustProcessSnapshotV2Magic {
		return nil, errInvalidRustProcessSnapshotV2
	}
	pathCount, ok := read()
	if !ok {
		return nil, errInvalidRustProcessSnapshotV2
	}
	entryCount, ok := read()
	if !ok {
		return nil, errInvalidRustProcessSnapshotV2
	}
	// Each path needs at least its u32 length and each entry at least inode and
	// owner count, so refuse impossible counts before allocating anything.
	if uint64(pathCount)*4+uint64(entryCount)*8 > uint64(len(blob)) {
		return nil, errInvalidRustProcessSnapshotV2
	}
	executables := make([]string, int(pathCount))
	for i := range executables {
		size, ok := read()
		if !ok || uint64(size) > uint64(len(blob)) {
			return nil, errInvalidRustProcessSnapshotV2
		}
		executables[i] = string(blob[:size])
		blob = blob[size:]
	}
	entries := make(map[uint32][]string, int(entryCount))
	// The chunk is allocated on first use and shared by consecutive entries;
	// ownerChunkUsed is the next free slot.
	var ownerChunk []string
	ownerChunkUsed := rustProcessOwnerChunkSize
	allocateOwners := func(count uint32) []string {
		if count == 0 {
			return []string{}
		}
		if count > rustProcessOwnerChunkSize {
			return make([]string, int(count))
		}
		if ownerChunkUsed+int(count) > rustProcessOwnerChunkSize {
			ownerChunk = make([]string, rustProcessOwnerChunkSize)
			ownerChunkUsed = 0
		}
		begin := ownerChunkUsed
		ownerChunkUsed += int(count)
		return ownerChunk[begin:ownerChunkUsed:ownerChunkUsed]
	}
	for range entryCount {
		inode, ok := read()
		if !ok {
			return nil, errInvalidRustProcessSnapshotV2
		}
		count, ok := read()
		if !ok || uint64(count)*4 > uint64(len(blob)) {
			return nil, errInvalidRustProcessSnapshotV2
		}
		if _, exists := entries[inode]; exists {
			return nil, errInvalidRustProcessSnapshotV2
		}
		owners := allocateOwners(count)
		for i := range owners {
			id, ok := read()
			if !ok || id >= pathCount {
				return nil, errInvalidRustProcessSnapshotV2
			}
			owners[i] = executables[id]
		}
		entries[inode] = owners
	}
	if len(blob) != 0 {
		return nil, errInvalidRustProcessSnapshotV2
	}
	return entries, nil
}

func buildProcessPathsSelected(uid uint32) (map[uint32][]string, error) {
	return scanProcessPathsRust2(pathProc, uid)
}
