//go:build linux && !with_lx_rust_proc

package process

func buildProcessPathsSelected(uid uint32) (map[uint32][]string, error) {
	return buildProcessPaths(uid)
}
