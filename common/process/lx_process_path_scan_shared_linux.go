//go:build linux

package process

func (s *linuxSearcher) readProcessPaths(uid uint32) (map[uint32][]string, error) {
	paths, err := buildProcessPaths(uid)
	if err != nil {
		return nil, err
	}
	s.processPathCache.Add(uid, &uidProcessPaths{entries: paths})
	return paths, nil
}
