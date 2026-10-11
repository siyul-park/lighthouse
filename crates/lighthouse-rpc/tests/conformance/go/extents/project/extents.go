package extents

// Limit caps the work.
//
// It is documented over several lines.
const Limit = 3

// A detached comment, a blank line away, belongs to nobody.

var plain = 1 // trailing note

// Mode is a group of constants.
const (
	// Fast skips the checks.
	Fast = iota
	Slow // checks everything
)

var (
	left, right = 1, 2
	only        = 3
)

// Service does the work.
type Service struct {
	// name identifies the service.
	name string `json:"name"`
	size int
}

// Reader reads.
type Reader interface {
	// Read reads once.
	Read() int
}

// Run runs the service.
func (s *Service) Run() int {
	return s.size + Limit
}

func helper() int { return plain }

type (
	// Pair holds two values.
	Pair struct{ a, b int }
	Solo struct{}
)
