/*
Helper functions for diagnosing memory problems.
*/

package main

import (
	"fmt"
	"runtime"
)

func SprintlnInterestingMemStats(mem *runtime.MemStats) string {
	return fmt.Sprintln(
		"alloc", HumanReadable(mem.Alloc, "B"),
		"totall", HumanReadable(mem.TotalAlloc, "B"),
		"sys", HumanReadable(mem.Sys, "B"),
		"halloc", HumanReadable(mem.HeapAlloc, "B"),
		"hsys", HumanReadable(mem.HeapSys, "B"),
		"huse", HumanReadable(mem.HeapInuse, "B"),
		"suse", HumanReadable(mem.StackInuse, "B"),
		"ssys", HumanReadable(mem.StackSys, "B"),
		"nextgc", HumanReadable(mem.NextGC, ""))
}

