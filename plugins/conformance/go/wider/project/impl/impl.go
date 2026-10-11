package impl

import "fmt"

type File struct{}

// Close closes.
func (f *File) Close() error { return nil }

func (f *File) Flush() {}

type source struct{}

func (source) Read(p []byte) (int, error) { return fmt.Println(len(p)) }

func (source) Other() {}
