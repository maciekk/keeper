/*
Helper functions for diagnosing memory problems.
*/

package main

import (
	"fmt"
	"github.com/maciekk/keeper/pretty"
	"runtime"
)

func SprintlnInterestingMemStats(mem *runtime.MemStats) string {
	return fmt.Sprintln(
		"alloc", pretty.HumanReadable(mem.Alloc, "B"),
		"totall", pretty.HumanReadable(mem.TotalAlloc, "B"),
		"sys", pretty.HumanReadable(mem.Sys, "B"),
		"halloc", pretty.HumanReadable(mem.HeapAlloc, "B"),
		"hsys", pretty.HumanReadable(mem.HeapSys, "B"),
		"huse", pretty.HumanReadable(mem.HeapInuse, "B"),
		"suse", pretty.HumanReadable(mem.StackInuse, "B"),
		"ssys", pretty.HumanReadable(mem.StackSys, "B"),
		"nextgc", pretty.HumanReadable(mem.NextGC, ""))
}
