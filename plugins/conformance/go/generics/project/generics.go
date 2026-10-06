package generics

// Number is a numeric constraint.
type Number interface {
	~int | ~float64
}

// Stack holds values of one type.
type Stack[T any] struct {
	items []T
}

// Push adds an item.
func (s *Stack[T]) Push(item T) { s.items = append(s.items, item) }

// Len counts the items.
func (s *Stack[T]) Len() int { return len(s.items) }

// Map applies f to every item.
func Map[T, U any](items []T, f func(T) U) []U {
	out := make([]U, 0, len(items))
	for _, item := range items {
		out = append(out, f(item))
	}
	return out
}

// Sum adds numbers.
func Sum[T Number](items ...T) T {
	var total T
	for _, item := range items {
		total += item
	}
	return total
}

func lengths(words []string) []int {
	return Map[string, int](words, func(w string) int { return len(w) })
}

func total() float64 { return Sum(1.5, 2.5) }

func fill() int {
	s := &Stack[int]{}
	s.Push(1)
	return s.Len()
}
