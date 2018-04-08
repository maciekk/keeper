//
// How to properly set up this project:
// - don't forget to put this project in proper Go structure: FOO/src/...
//   (where FOO is something like ~/go); make sure FOO is in $GOPATH
//   environment variable
//
// Next steps:
// - do homedir expansion on filenames in batch file
// - store some useful info in the SFV comment header:
//   #files, #dirs, #bytes total, date of record
// - try out 'gocheck', which seems to have test fixtures:
//   https://code.google.com/r/splade2009-camlistore/source/browse/lib/go/launchpad.net/gocheck/?r=a7b2879e439c76aa6b198a7b88baf58f5fc9f375
// - does Verify handle correctly the case when a file listed in SFV is missing?
// - should Verify report that there are more files now than in SFV?
// - fix: sfv scanner should ignore .sfv files (seems pointless to include them)
// - make 'snapshot' capable of copying: ensure prior SFV of src, copy,
//   validate SFV of dest
// - add fns to take MemStats snapshots, and every so often report deltas
// - factor out cmd sorting into generic routine GetMapKeys
//

package main

import (
	"bufio"
	"flag"
	"io"
	"github.com/maciekk/keeper/elog"
	"github.com/maciekk/keeper/sfv"
	"os"
	"sort"
	"strings"
)

var flagBatchFile = flag.String("F", "", "file containing sequence of commands")

func HandleCheckSfv(args []string) {
	if len(args) != 1 {
		elog.Println("Incorrect # of args; syntax is:")
		elog.Println("  keeper check <sfv filename>")
		return
	}
	errors := sfv.SfvVerify(args[0])
	if len(errors) > 0 {
		elog.Println("")
		elog.Println("Summary of errors encountered:")
		for _, err := range errors {
			elog.Println(err)
		}
	} else {
		elog.Println("Success! No errors.")
	}
}

func HandleRecordSfv(args []string) {
	if len(args) != 2 {
		elog.Println("Incorrect # of args; syntax is:")
		elog.Println("  keeper record <src dir> <sfv filename>")
		return
	}
	sfv.SfvRecord(args[0], args[1])
}

func HandleSnapshot(args []string) {
	if len(args) != 2 {
		elog.Println("Incorrect # of args; syntax is:")
		elog.Println("  keeper snapshot <src dir> <repo dir>")
		return
	}

	src, repo := args[0], args[1]
	elog.Printf("Running 'snapshot': %s -> %s\n", src, repo)

	elog.Println("Not implemented yet.")
}

var dispatcher = map[string]func([]string) {
	"check": HandleCheckSfv,
	"record": HandleRecordSfv,
	"snapshot": HandleSnapshot,
}

func printSyntax() {
	elog.Println("Syntax: keeper <cmd> [<arg> ...]")
}

func printKnownCommands() {
	cmds := make([]string, len(dispatcher))
	i := 0
	for k := range dispatcher {
		cmds[i] = k
		i++
	}
	sort.Strings(cmds)
	elog.Println("Known commands:")
	for _, c := range cmds {
		elog.Println("  ", c)
	}
}

func commandProcessor(chanCmd <-chan []string, chanQuit <-chan bool) {
	for {
		select {
		case args := <-chanCmd:
			cmd, args := args[0], args[1:]
			handler, ok := dispatcher[cmd]
			if !ok {
				elog.Println("Unsupported command: " + cmd)
				printKnownCommands()
				os.Exit(-2)
			}
			handler(args)
		case  <- chanQuit:
			return
		}
	}
}

func batchFileReader(filename string,
	chanOut chan<- []string, chanDone chan<- bool) {

	f, err := os.Open(filename)
	if err != nil {
		elog.Fatal(err)
	}
	defer f.Close()

    r := bufio.NewReaderSize(f, 4 * 1024)

	for {
		line, isPrefix, err := r.ReadLine()
		if err == io.EOF {
			break
		}
		if err != nil {
			elog.Fatal(err)
		}
		if isPrefix {
			elog.Fatal("Large line reading not yet supported.")
		}
		lineTrimmed := strings.TrimSpace(string(line))
		if lineTrimmed[0] == ';' || lineTrimmed == "" {
			// Skip comments & blank lines.
			continue
		}
		chanOut <- strings.Fields(lineTrimmed)
	}
	chanDone <- true
}

func main() {
	flag.Parse()
	args := flag.Args()

	if len(args) == 0 && *flagBatchFile == "" {
		printSyntax()
		os.Exit(-1)
	}

	chanCmd := make(chan []string)
	chanQuit := make(chan bool)
	chanDone := make(chan bool)
	go commandProcessor(chanCmd, chanQuit)
	if *flagBatchFile != "" {
		elog.Println("Processing batch instruction file",
			*flagBatchFile)
		go batchFileReader(*flagBatchFile, chanCmd, chanDone)
		// Wait for all the batch file commands to get processed.
		<-chanDone
	}

	// Also process the command on commandline, if present.
	if len(args) > 0 {
		chanCmd <- args
	}

	// Quit goroutines.
	chanQuit <- true
}
