package signatures

// Request is taken by Build only.
type Request struct {
	ID    int
	Name  string
	Tags  []string
	Limit *int
	Hook  func()
	Next  any
	Done  chan int
	Index map[string]int
}

// Pair is returned by Split only.
type Pair struct{ A, B int }

func Build(r Request, n int) int { return r.ID + n }

func Split() (Pair, error) { return Pair{}, nil }
