//
// "Echoing log": logs messages to log file and stdout
//
// NOTE: we use 'log' and 'fmt' together, rather than a MultiWriter
// approach from say:
//  https://groups.google.com/forum/?fromgroups=#!topic/golang-nuts/BCCLURRPA8o
// because we want different options on each (e.g., no timestamp on stdout)
//

package elog

import (
	"fmt"
	"log"
	"os"
	"strings"
)

func init() {
	fl, err := os.OpenFile("keeper.log", os.O_APPEND|os.O_CREATE, 0640)
	if err != nil {
		log.Fatal(err)
	}
	log.SetOutput(fl)
	// Mark the start of a new run of program (since we append to log).
	log.Println(strings.Repeat("-", 40))
}

func Printf(format string, v ...interface{}) {
	fmt.Printf(format, v...)
	log.Printf(format, v...)
}

func Println(v ...interface{}) {
	fmt.Println(v...)
	log.Println(v...)
}

func Fatal(v ...interface{}) {
	fmt.Println(v...)
	log.Fatal(v...)
}