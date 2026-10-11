package events

import (
	"io/fs"
	"os"
	"time"
)

type Base struct{}

func (Base) Close() error { return nil }

// Shielded replaces the Close it gets from Base.
type Shielded struct{ Base }

func (w Shielded) Close() error { return w.Base.Close() }

type Leaf struct{}

func (Leaf) Name() string { return "" }

// Holder gets Name from Leaf; the interface is satisfied by Holder.
type Holder struct{ Leaf }

type namer interface{ Name() string }

func Use(n namer) string { return n.Name() }

func Pass() string { return Use(Holder{}) }

type Simple struct{}

func (Simple) Other() int { return 0 }

// entry is only ever used as a value of an interface of another package.
type entry struct{}

func (entry) Name() string       { return "" }
func (entry) Size() int64        { return 0 }
func (entry) Mode() fs.FileMode  { return 0 }
func (entry) ModTime() time.Time { return time.Time{} }
func (entry) IsDir() bool        { return false }
func (entry) Sys() any           { return nil }

func Stat() fs.FileInfo {
	info, err := os.Stat("x")
	if err != nil {
		return entry{}
	}
	return info
}
