
package sfv

import (
	"fmt"
	"io/ioutil"
	"log"
	"os"
	"path/filepath"
	"testing"
)

func makeConstantSlice(length int, value byte) []byte {
	data := make([]byte, length, length)
	for i := 0; i < length; i++ {
		data[i] = value
	}
	return data
}

func makeRisingIntsSlice(length int) []byte {
	data := make([]byte, length, length)
	for i := 0; i < length; i++ {
		data[i] = byte(i % 256)
	}
	return data
}

func makeStringSlice(s string) []byte {
	data := make([]byte, len(s), len(s))
	for i := 0; i < len(s); i++ {
		data[i] = byte(s[i])
	}
	return data
}

type testHelper struct {
	t *testing.T
}

func (h testHelper) Error(v ...interface{}) {
	h.t.Error(v...)
}

func (h testHelper) writeTempFile(content []byte) string {
	return h.writeTempFileAt("", content)
}

func (h testHelper) writeTempFileAt(path string, content []byte) string {
	f, err := ioutil.TempFile(path, "TestFile")
	if err != nil {
		h.Error(err)
	}
	defer f.Close()
	filename := f.Name()
	_, err = f.Write(content)
	if err != nil {
		h.Error(err)
	}
	return filename
}

func (h testHelper) writeFile(filename string, path string, content []byte) {
	f, err := os.Create(filepath.Join(path, filename))
	if err != nil {
		h.Error(err)
	}
	defer f.Close()
	_, err = f.Write(content)
	if err != nil {
		h.Error(err)
	}
}

// This always creates the same directory and contents.
func (h testHelper) makeTestDir() (path string) {
	path, err := ioutil.TempDir("", "TestDir")
	if err != nil {
		log.Fatal(err)
	}
	
	// Now populate it with interesting files.
	// - zero byte file
	h.writeFile("0_byte_file", path, makeRisingIntsSlice(0))
	
	// - one byte file, zero value
	h.writeFile("1_byte_file_0_value", path,
		makeConstantSlice(1, 0))

	// - one byte file, 128 value
	h.writeFile("1_byte_file_128_value", path,
		makeConstantSlice(1, 128))

	// - ascending ints file, 10 bytes
	h.writeFile("10_byte_file_rising_ints", path,
		makeRisingIntsSlice(10))

	// - ascending ints file, 100 bytes
	h.writeFile("100_byte_file_rising_ints", path,
		makeRisingIntsSlice(100))

	// - all zeros, 100 byte file
	h.writeFile("100_byte_file_0_value", path,
		makeConstantSlice(100, 0))
	
	// - all 255, 100 byte file
	h.writeFile("100_byte_file_255_value", path,
		makeConstantSlice(100, 255))

	// - Hello World! file
	h.writeFile("hello_world_file_plain", path,
		makeStringSlice("Hello World!"))

	// - Hello World!\n file
	h.writeFile("hello_world_file_nl", path,
		makeStringSlice("Hello World!\n"))

	// - Hello World!\r\n file
	h.writeFile("hello_world_file_crnl", path,
		makeStringSlice("Hello World!\r\n"))

	return path
}

// Intended to be called immediately after makeRisingIntsFile(), using 'defer'.
func (h testHelper) deleteFile(filename string) {
	err := os.Remove(filename)
	if err != nil {
		h.Error(err)
	}
}

func (h testHelper) deleteDir(path string) {
	err := os.RemoveAll(path)
	if err != nil {
		h.Error(err)
	}
}

func Test_ComputeFileChecksumMissingFile(t *testing.T) {
	crc := NewCrc32Buffer(1024)
	checksum, ok := crc.ComputeFileChecksum(
		"Test_ComputeFileChecksumMissingFile_non_existent_file")
	if (ok == true || checksum != 0) {
		t.Error("Incorrect handling of missing file.")
	}
}

func Test_ComputeFileChecksumBasic(t *testing.T) {
	h := testHelper{t}
	crc := NewCrc32Buffer(1024)
	for i := 0; i < 100; i++ {
		filename := h.writeTempFile(makeRisingIntsSlice(i))
		defer h.deleteFile(filename)
		checksum, ok := crc.ComputeFileChecksum(filename)
		if ok == false {
			h.Error("Encountered error")
		}
		log.Printf("%d: %s -> %08X\n", i, filename, checksum)
	}
	var goodChecksums = map [int]uint32 {
		0: 0,
		1: 0xd202ef8d,
		2: 0x36de2269,
		10: 0x456cd746,
		99: 0xae149478,
	}
	for i := range goodChecksums {
		filename := h.writeTempFile(makeRisingIntsSlice(i))
		defer h.deleteFile(filename)
		checksum, ok := crc.ComputeFileChecksum(filename)
		if ok == false {
			h.Error("Encountered error calling crc32File()")
		}
		if checksum != goodChecksums[i] {
			h.Error("Incorrect checksum for i == ", i)
		}
	}
}

func Test_ComputeFileChecksumMultiBlock(t *testing.T) {
	h := testHelper{t}
	const kFileSize int64 = 250
	crc := NewCrc32Buffer(kFileSize + 10)
	filename := h.writeTempFile(makeRisingIntsSlice(int(kFileSize)))
	defer h.deleteFile(filename)
	goodChecksum, ok := crc.ComputeFileChecksum(filename)
	if ok == false {
		h.Error("Encountered error with ComputeFileChecksum()")
	}

	bufferSizesToTry := [...]int64{
		kFileSize + 1,
		kFileSize,
		kFileSize - 1,
		kFileSize - 2,
		kFileSize / 2,
		2,
		1,
	}
	for _, buffSize := range(bufferSizesToTry) {
		crc = NewCrc32Buffer(int64(buffSize))
		checksum, ok := crc.ComputeFileChecksum(filename)
		if ok == false || checksum != goodChecksum {
			h.Error("Incorrect ComputeFileChecksum() result with buffer size",
				buffSize)
		} else {
			fmt.Printf("CRC OK: crc32=0x%08x, buffSize=%d\n", checksum, buffSize)
		}
	}
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func Test_SfvRecord(t *testing.T) {
	h := testHelper{t}
	path := h.makeTestDir()
	defer h.deleteDir(path)
	sfvFilePath := filepath.Join(path, "test.sfv")
	success := SfvRecord(path, sfvFilePath)
	if success == false {
		h.Error("Call to SfvRecord() failed for some reason.")
	}
	
	expectedSfv := `; Generated by keeper.go
;
0_byte_file 00000000
100_byte_file_0_value 9988C6CA
100_byte_file_255_value 03D28681
100_byte_file_rising_ints 58C932F5
10_byte_file_rising_ints 456CD746
1_byte_file_0_value D202EF8D
1_byte_file_128_value 3FBA6CAD
hello_world_file_crnl 85892AE0
hello_world_file_nl 7D14DDDD
hello_world_file_plain 1C291CA3
`
	buffer_size := len(expectedSfv) + 1
	buffer := make([]byte, buffer_size, buffer_size)
	f, err := os.Open(sfvFilePath)
	defer f.Close()
	if err != nil {
		h.Error("Trouble opening test.sfv:", err)
	}
	n, err := f.Read(buffer)
	if err != nil {
		h.Error("Trouble reading test.sfv:", err)
	}
	if n != len(expectedSfv) {
		h.Error("SFV has incorrect file size; expected:", len(expectedSfv),
			"actual is >=", n)
	}
	buffer = buffer[:n]
	mismatch := false
	length := min(len(expectedSfv), len(buffer))
	for i := 0; i < length; i++ {
		if byte(expectedSfv[i]) != buffer[i] {
			mismatch = true
			break
		}
	}
	if mismatch {
		h.Error("SFV payload does not match expectations")
		fmt.Println("Expected:", expectedSfv)
		fmt.Println("Actual:", string(buffer))
	}
}
