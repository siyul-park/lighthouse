package impl

// File is a sink that never names the interface.
type File struct{}

// Close closes.
func (f *File) Close() error { return nil }

// Flush flushes.
func (f *File) Flush() {}
