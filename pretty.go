// Routines for pretty printing numbers.

package main

import (
	"bytes"
	"fmt"
	"log"
)

func Commify(v int64) string {
	// TODO: consider negative values
	if v == 0 {
		return "0"
	}

	is_negative := false
	if v < 0 {
		is_negative = true
		v *= -1
	}
	
	var chunks []int
	for v > 0 {
		chunks = append(chunks, int(v % 1000))
		v = int64(v / 1000)
	}

	var result bytes.Buffer
	if is_negative {
		result.WriteString("-")
	}
	// There must be at least 1 chunk, as we fast-returned on v == 0.
	result.WriteString(fmt.Sprintf("%3d", chunks[len(chunks)-1]))

	for i := len(chunks)-2; i >= 0; i-- {
		result.WriteString(fmt.Sprintf(",%03d", chunks[i]))
	}
	return result.String()
}

func HumanReadable(v uint64, unit string) string {
	const divisor = 1024.0
	divs := 0
	value := float64(v)
	for value >= divisor {
		value /= divisor
		divs++
	}
	var si_prefix string
	switch divs {
	case 0:
		si_prefix = ""
	case 1:
		si_prefix = "k"
	case 2:
		si_prefix = "M"
	case 3:
		si_prefix = "G"
	case 4:
		si_prefix = "T"
	case 5:
		si_prefix = "P"
	case 6:
		si_prefix = "E"
	default:
		si_prefix = "?"
	}
	return fmt.Sprintf("%.3f %s%s", value, si_prefix, unit)
}

// Contracts the input string 's' to be at most length 'max_width'.
// It does this by replacing excess characters in the middle of the
// string with an ellipsis ("...").
func CollapseMiddle(s string, max_width int) string {
	if len(s) <= max_width {
		return s
	}
	excess_chars := len(s) - max_width + 3  // ellipsis takes 3 chars
	var start int = (max_width - 3) / 2
	result := s[:start] + "..." + s[start + excess_chars:]
	// TODO: remove this temporary diagnostic code, once have unittests
	if len(result) > max_width || len(result) < max_width - 1 {
		log.Fatal("CollapseMiddle() logic error: ", max_width, " ", len(result))
	}
	return result
}

