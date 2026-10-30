//go:build linux

package process

import (
	"net"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"syscall"
	"testing"
)

func processPathSockets(t testing.TB, count int) []uint32 {
	t.Helper()
	inodes := make([]uint32, count)
	for index := range inodes {
		conn, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		raw, err := conn.SyscallConn()
		if err != nil {
			t.Fatal(err)
		}
		var link string
		var linkErr error
		err = raw.Control(func(fd uintptr) {
			link, linkErr = os.Readlink(filepath.Join("/proc/self/fd", strconv.FormatUint(uint64(fd), 10)))
		})
		if err != nil || linkErr != nil {
			t.Fatalf("read socket inode: control=%v link=%v", err, linkErr)
		}
		inode, ok := parseSocketInode([]byte(link))
		if !ok {
			t.Fatalf("unusable socket inode: %q", link)
		}
		inodes[index] = inode
	}
	return inodes
}

func TestProcessPathScanConcurrentSockets(t *testing.T) {
	inodes := processPathSockets(t, 8)
	searcher, err := NewSearcher(Config{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = searcher.Close() })
	linux := searcher.(*linuxSearcher)
	var workers sync.WaitGroup
	start := make(chan struct{})
	for _, inode := range inodes {
		workers.Add(1)
		go func() {
			defer workers.Done()
			<-start
			paths, err := linux.findProcessPaths(inode, uint32(os.Getuid()))
			if err != nil || len(paths) == 0 {
				t.Errorf("socket owner missing: inode=%d paths=%v err=%v", inode, paths, err)
			}
		}()
	}
	close(start)
	workers.Wait()

	// A new socket absent from the cached snapshot must still be discovered.
	newInode := processPathSockets(t, 1)[0]
	paths, err := linux.findProcessPaths(newInode, uint32(os.Getuid()))
	if err != nil || len(paths) == 0 {
		t.Fatalf("new socket was hidden by an earlier scan: paths=%v err=%v", paths, err)
	}
	// A socket's creation UID can differ from its current holder's UID. Keep
	// the existing all-users fallback, rather than treating a miss as final.
	paths, err = linux.findProcessPaths(newInode, uint32(os.Getuid())+1)
	if err != nil || len(paths) == 0 {
		t.Fatalf("all-users fallback lost a socket: paths=%v err=%v", paths, err)
	}
}

func BenchmarkProcessPathScanBurst(b *testing.B) {
	for _, workers := range []int{1, 4, 8} {
		b.Run(strconv.Itoa(workers), func(b *testing.B) {
			inodes := processPathSockets(b, workers)
			uid := uint32(os.Getuid())
			b.ReportAllocs()
			var before, after syscall.Rusage
			if err := syscall.Getrusage(syscall.RUSAGE_SELF, &before); err != nil {
				b.Fatal(err)
			}
			b.ResetTimer()
			for range b.N {
				searcher, err := NewSearcher(Config{})
				if err != nil {
					b.Fatal(err)
				}
				linux := searcher.(*linuxSearcher)
				var group sync.WaitGroup
				start := make(chan struct{})
				for _, inode := range inodes {
					group.Add(1)
					go func() {
						defer group.Done()
						<-start
						paths, err := linux.findProcessPaths(inode, uid)
						if err != nil || len(paths) == 0 {
							b.Errorf("socket owner missing: paths=%v err=%v", paths, err)
						}
					}()
				}
				close(start)
				group.Wait()
				_ = searcher.Close()
			}
			b.StopTimer()
			if err := syscall.Getrusage(syscall.RUSAGE_SELF, &after); err != nil {
				b.Fatal(err)
			}
			cpuNanos := (after.Utime.Nano() - before.Utime.Nano()) + (after.Stime.Nano() - before.Stime.Nano())
			b.ReportMetric(float64(cpuNanos)/float64(b.N), "cpu-ns/burst")
		})
	}
}

// Compare with the pre-change algorithm inside one binary: compiler, object
// layout, runtime and process population are identical for both alternatives.
// Both cases keep one searcher alive, matching its production lifetime.
func BenchmarkProcessPathScanSameBinary(b *testing.B) {
	modes := []string{"reference", "candidate"}
	if os.Getenv("MAGICNET_BENCH_ORDER") == "candidate-first" {
		modes[0], modes[1] = modes[1], modes[0]
	}
	for _, workers := range []int{1, 4, 8} {
		for _, mode := range modes {
			b.Run(mode+"/"+strconv.Itoa(workers), func(b *testing.B) {
				inodes := processPathSockets(b, workers)
				uid := uint32(os.Getuid())
				searcher, err := NewSearcher(Config{})
				if err != nil {
					b.Fatal(err)
				}
				defer searcher.Close()
				linux := searcher.(*linuxSearcher)
				lookup := (*linuxSearcher).findProcessPaths
				if mode == "reference" {
					lookup = findProcessPathsBeforeCoalescing
				}
				var before, after syscall.Rusage
				if err := syscall.Getrusage(syscall.RUSAGE_SELF, &before); err != nil {
					b.Fatal(err)
				}
				b.ReportAllocs()
				b.ResetTimer()
				for range b.N {
					linux.processPathCache.Purge()
					var group sync.WaitGroup
					start := make(chan struct{})
					for _, inode := range inodes {
						group.Add(1)
						go func() {
							defer group.Done()
							<-start
							paths, err := lookup(linux, inode, uid)
							if err != nil || len(paths) == 0 {
								b.Errorf("socket owner missing: paths=%v err=%v", paths, err)
							}
						}()
					}
					close(start)
					group.Wait()
				}
				b.StopTimer()
				if err := syscall.Getrusage(syscall.RUSAGE_SELF, &after); err != nil {
					b.Fatal(err)
				}
				cpuNanos := after.Utime.Nano() - before.Utime.Nano() + after.Stime.Nano() - before.Stime.Nano()
				b.ReportMetric(float64(cpuNanos)/float64(b.N), "cpu-ns/burst")
			})
		}
	}
}

// This is the exact cache/UID fallback sequence before the change. Only the
// scan call bypasses loadProcessPaths; it is a benchmark reference, not a
// second production implementation.
func findProcessPathsBeforeCoalescing(s *linuxSearcher, inode, uid uint32) ([]string, error) {
	for _, scanUID := range []uint32{uid, processPathsAllUsers} {
		if cached, ok := s.processPathCache.Get(scanUID); ok {
			if paths, found := cached.entries[inode]; found {
				return paths, nil
			}
		}
		paths, err := s.readProcessPaths(scanUID)
		if err != nil {
			return nil, err
		}
		if found, ok := paths[inode]; ok {
			return found, nil
		}
	}
	return nil, ErrNotFound
}
