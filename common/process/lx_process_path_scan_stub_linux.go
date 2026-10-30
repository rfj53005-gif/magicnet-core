//go:build linux && !with_ebpf

package process

type processPathScanLocks struct{}

func (s *linuxSearcher) loadProcessPaths(uid, _ uint32) (map[uint32][]string, error) {
	return s.readProcessPaths(uid)
}
