package embedded

import "io"

// Logger writes lines.
type Logger interface {
	Log(msg string)
}

// Closer releases resources.
type Closer interface {
	Close() error
}

// LogCloser does both.
type LogCloser interface {
	Logger
	Closer
}

// Base provides the shared behavior.
type Base struct{ prefix string }

// Log writes a line.
func (b *Base) Log(msg string) { _ = b.prefix + msg }

// Close does nothing.
func (b *Base) Close() error { return nil }

// Service embeds its base by pointer and a reader from another package.
type Service struct {
	*Base
	io.Reader
	name string
}

func run(s *Service) error {
	s.Log("start")
	return s.Close()
}

func peek(s *Service) string { return s.name + s.prefix }

func build() *Service { return &Service{Base: &Base{prefix: "> "}, name: "svc"} }

func closeAll(items []LogCloser) {
	for _, item := range items {
		_ = item.Close()
	}
}
