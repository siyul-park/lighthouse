package api

// Sink takes what is written to it.
type Sink interface {
	Close() error
	Flush()
}
