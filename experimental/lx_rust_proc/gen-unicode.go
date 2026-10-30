//go:build ignore

package main

import (
	"fmt"
	"unicode"
)

func main() {
	fmt.Printf("// Generated from pinned Go unicode.Digit, Unicode %s.\n", unicode.Version)
	fmt.Println("pub const DIGITS: &[(u32, u32, u32)] = &[")
	for _, r := range unicode.Digit.R16 {
		fmt.Printf("    (0x%x, 0x%x, %d),\n", r.Lo, r.Hi, r.Stride)
	}
	for _, r := range unicode.Digit.R32 {
		fmt.Printf("    (0x%x, 0x%x, %d),\n", r.Lo, r.Hi, r.Stride)
	}
	fmt.Println("]; ")
}
