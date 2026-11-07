package api

// sink is an interface only a test of the package declares.
type sink interface {
	Close() error
	Flush()
}
