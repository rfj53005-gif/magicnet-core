//go:build linux && with_ebpf

package process

import "sync"

// Bound lock storage independently of the number of UIDs observed. The normal
// cache-hit path does not acquire these locks. Only cache misses for the same
// shard serialize, so simultaneous connections can reuse a completed scan.
type processPathScanLocks struct {
	shards [64]sync.Mutex
}

func (s *linuxSearcher) loadProcessPaths(uid, targetInode uint32) (map[uint32][]string, error) {
	lock := &s.processPathScanLocks.shards[uid%uint32(len(s.processPathScanLocks.shards))]
	lock.Lock()
	defer lock.Unlock()

	// A preceding scan may cover this socket. Do not reuse a snapshot missing
	// the requested inode: a later connection may have been created after it.
	if cached, ok := s.processPathCache.Get(uid); ok {
		if _, found := cached.entries[targetInode]; found {
			return cached.entries, nil
		}
	}
	return s.readProcessPaths(uid)
}
