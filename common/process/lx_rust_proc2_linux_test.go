//go:build linux && cgo && with_lx_rust_proc && with_lx_rust_proc_rust2

package process

import (
	"encoding/binary"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"testing"
)

type rust2SnapshotEntry struct {
	inode  uint32
	owners []uint32
}

func buildRust2Snapshot(executables []string, entries []rust2SnapshotEntry) []byte {
	blob := make([]byte, 0, 12+len(executables)*8+len(entries)*16)
	blob = binary.LittleEndian.AppendUint32(blob, rustProcessSnapshotV2Magic)
	blob = binary.LittleEndian.AppendUint32(blob, uint32(len(executables)))
	blob = binary.LittleEndian.AppendUint32(blob, uint32(len(entries)))
	for _, executable := range executables {
		blob = binary.LittleEndian.AppendUint32(blob, uint32(len(executable)))
		blob = append(blob, executable...)
	}
	for _, entry := range entries {
		blob = binary.LittleEndian.AppendUint32(blob, entry.inode)
		blob = binary.LittleEndian.AppendUint32(blob, uint32(len(entry.owners)))
		for _, owner := range entry.owners {
			blob = binary.LittleEndian.AppendUint32(blob, owner)
		}
	}
	return blob
}

func TestDecodeRustProcessSnapshotV2(t *testing.T) {
	executables := []string{"/app/10\xff", "/app/2", "/usr/bin/daemon"}
	blob := buildRust2Snapshot(executables, []rust2SnapshotEntry{
		{inode: 42, owners: []uint32{0, 1}},
		{inode: 43, owners: []uint32{2}},
	})
	decoded, err := decodeRustProcessSnapshotV2(blob)
	if err != nil {
		t.Fatal(err)
	}
	want := map[uint32][]string{
		42: {"/app/10\xff", "/app/2"},
		43: {"/usr/bin/daemon"},
	}
	if !reflect.DeepEqual(decoded, want) {
		t.Fatalf("decoded snapshot = %#v, want %#v", decoded, want)
	}

	empty, err := decodeRustProcessSnapshotV2(buildRust2Snapshot(nil, nil))
	if err != nil {
		t.Fatal(err)
	}
	if len(empty) != 0 {
		t.Fatalf("empty snapshot decoded to %#v", empty)
	}
}

func TestDecodeRustProcessSnapshotV2Rejections(t *testing.T) {
	executables := []string{"/app/one"}
	valid := buildRust2Snapshot(executables, []rust2SnapshotEntry{
		{inode: 7, owners: []uint32{0}},
	})
	tests := []struct {
		name string
		blob []byte
	}{
		{
			name: "Empty",
			blob: nil,
		},
		{
			name: "ShortHeader",
			blob: valid[:11],
		},
		{
			name: "BadMagic",
			blob: func() []byte {
				blob := slices.Clone(valid)
				binary.LittleEndian.PutUint32(blob, rustProcessSnapshotV2Magic+1)
				return blob
			}(),
		},
		{
			name: "DuplicateInode",
			blob: buildRust2Snapshot(executables, []rust2SnapshotEntry{
				{inode: 7, owners: []uint32{0}},
				{inode: 7, owners: []uint32{0}},
			}),
		},
		{
			name: "OutOfRangePathID",
			blob: buildRust2Snapshot(executables, []rust2SnapshotEntry{
				{inode: 7, owners: []uint32{0}},
				{inode: 8, owners: []uint32{1}},
			}),
		},
		{
			name: "TrailingByte",
			blob: append(slices.Clone(valid), 0),
		},
		{
			name: "HugeCounts",
			blob: func() []byte {
				blob := buildRust2Snapshot(nil, nil)
				binary.LittleEndian.PutUint32(blob[4:], ^uint32(0))
				binary.LittleEndian.PutUint32(blob[8:], ^uint32(0))
				return blob
			}(),
		},
		{
			name: "HugePathCount",
			blob: func() []byte {
				blob := buildRust2Snapshot(nil, nil)
				binary.LittleEndian.PutUint32(blob[4:], ^uint32(0))
				return blob
			}(),
		},
		{
			name: "HugeEntryCount",
			blob: func() []byte {
				blob := buildRust2Snapshot(nil, nil)
				binary.LittleEndian.PutUint32(blob[8:], ^uint32(0))
				return blob
			}(),
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if _, err := decodeRustProcessSnapshotV2(test.blob); err == nil {
				t.Fatalf("invalid snapshot was accepted: %#v", test.blob)
			}
		})
	}
}

func TestDecodeRustProcessSnapshotV2Truncation(t *testing.T) {
	blob := buildRust2Snapshot([]string{"/app/1", "/app/2"}, []rust2SnapshotEntry{
		{inode: 7, owners: []uint32{0, 1}},
		{inode: 8, owners: []uint32{1}},
	})
	for cut := range len(blob) {
		if _, err := decodeRustProcessSnapshotV2(blob[:cut]); err == nil {
			t.Fatalf("snapshot truncated at %d/%d bytes was accepted", cut, len(blob))
		}
	}
}

func TestDecodeRustProcessSnapshotV2ZeroHolders(t *testing.T) {
	blob := buildRust2Snapshot([]string{"/app/one"}, []rust2SnapshotEntry{
		{inode: 9, owners: nil},
	})
	decoded, err := decodeRustProcessSnapshotV2(blob)
	if err != nil {
		t.Fatal(err)
	}
	owners, found := decoded[9]
	if !found {
		t.Fatal("zero-holder inode missing")
	}
	if owners == nil || len(owners) != 0 || cap(owners) != 0 {
		t.Fatalf("zero-holder owners = %#v (len=%d cap=%d), want non-nil empty", owners, len(owners), cap(owners))
	}
}

func chunkRust2SnapshotFixture() ([]string, []rust2SnapshotEntry, []byte) {
	const executableCount = 80
	executables := make([]string, executableCount)
	for i := range executables {
		executables[i] = fmt.Sprintf("/app/%d", i)
	}
	ids := func(from, count uint32) []uint32 {
		result := make([]uint32, count)
		for i := range result {
			result[i] = from + uint32(i)
		}
		return result
	}
	entries := []rust2SnapshotEntry{
		// Two holders, then three holders in the same 32-string chunk.
		{inode: 1, owners: ids(0, 2)},
		{inode: 2, owners: ids(2, 3)},
		// 5 used slots + 30 holders do not fit, so a fresh chunk is opened.
		{inode: 3, owners: ids(5, 30)},
		// 30 used slots + 4 holders do not fit either.
		{inode: 4, owners: ids(35, 4)},
		// More than one chunk: exact independent allocation.
		{inode: 5, owners: ids(39, 33)},
		// Zero holders takes no chunk slot.
		{inode: 6, owners: nil},
	}
	return executables, entries, buildRust2Snapshot(executables, entries)
}

func TestChunkRustProcessSnapshotV2(t *testing.T) {
	executables, entries, blob := chunkRust2SnapshotFixture()
	decoded, err := decodeRustProcessSnapshotV2(blob)
	if err != nil {
		t.Fatal(err)
	}
	if len(decoded) != len(entries) {
		t.Fatalf("decoded %d inodes, want %d", len(decoded), len(entries))
	}
	for _, entry := range entries {
		owners, found := decoded[entry.inode]
		if !found {
			t.Fatalf("inode %d missing", entry.inode)
		}
		if len(owners) != len(entry.owners) {
			t.Fatalf("inode %d owners len = %d, want %d", entry.inode, len(owners), len(entry.owners))
		}
		if cap(owners) != len(owners) {
			t.Fatalf("inode %d owners cap = %d, want len %d", entry.inode, cap(owners), len(owners))
		}
		want := make([]string, len(entry.owners))
		for i, id := range entry.owners {
			want[i] = executables[id]
		}
		if !slices.Equal(owners, want) {
			t.Fatalf("inode %d owners = %#v, want %#v", entry.inode, owners, want)
		}
	}
	if got := cap(decoded[5]); got != 33 {
		t.Fatalf("inode 5 (33 holders) cap = %d, want exact 33", got)
	}

	t.Run("NoAliasing", func(t *testing.T) {
		decoded, err := decodeRustProcessSnapshotV2(blob)
		if err != nil {
			t.Fatal(err)
		}
		baseline := make(map[uint32][]string, len(decoded))
		for inode, owners := range decoded {
			baseline[inode] = slices.Clone(owners)
		}
		mutated := map[uint32]string{
			1: "mutated/inode-1",
			2: "mutated/inode-2",
			3: "mutated/inode-3",
			4: "mutated/inode-4",
			5: "mutated/inode-5",
		}
		for inode, replacement := range mutated {
			decoded[inode][0] = replacement
		}
		for inode, want := range baseline {
			got := decoded[inode]
			start := 0
			if replacement, ok := mutated[inode]; ok {
				if got[0] != replacement {
					t.Fatalf("inode %d owner[0] = %q, want mutation %q", inode, got[0], replacement)
				}
				start = 1
			}
			if !slices.Equal(got[start:], want[start:]) {
				t.Fatalf("inode %d owners = %#v, want %#v", inode, got[start:], want[start:])
			}
		}
	})
}

func TestLXRustProcess2ScannerVariant(t *testing.T) {
	if variant := rust2ScannerVariant(); variant != 2 {
		t.Fatalf("linked Rust process scanner reports variant %d, want 2", variant)
	}
	if err := ensureRust2ScannerVariant(); err != nil {
		t.Fatal(err)
	}
}

func TestLXRustProcess2PathScan(t *testing.T) {
	inode := processPathSockets(t, 1)[0]
	executable, err := os.Readlink("/proc/self/exe")
	if err != nil {
		t.Fatal(err)
	}
	for _, uid := range []uint32{uint32(os.Getuid()), processPathsAllUsers} {
		paths, err := scanProcessPathsRust2(pathProc, uid)
		if err != nil {
			t.Fatal(err)
		}
		if !slices.Contains(paths[inode], executable) {
			t.Fatal("Rust2 scanner lost live socket owner")
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
	paths, err := scanProcessPathsRust2(root, processPathsAllUsers)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(paths, map[uint32][]string{42: {"/app/10\xff", "/app/2\xff"}}) {
		t.Fatal("Rust2 fixture order/dedup/raw bytes changed")
	}
	if _, err := scanProcessPathsRust2(filepath.Join(root, "missing"), processPathsAllUsers); !os.IsNotExist(err) {
		t.Fatal("Rust2 scan lost ENOENT")
	}
}

func FuzzDecodeRustProcessSnapshotV2(f *testing.F) {
	f.Add([]byte("MPS1\x00\x00\x00\x00\x00\x00\x00\x00"))
	f.Add([]byte("MPS1\xff\xff\xff\xff"))
	f.Fuzz(func(t *testing.T, blob []byte) { _, _ = decodeRustProcessSnapshotV2(blob) })
}
