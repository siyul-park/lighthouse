package blank

// I is implemented by T.
type I interface{ M() }

// A is part of the example.
type A interface{ M() }

// B is part of the example.
type B interface{ M() }

// T implements I.
type T struct{}

// M is part of the example.
func (T) M() {}

var x T

// The assertions below need no name.
var _ I = T{}

var _ I = (*T)(nil)

var (
	_ A = x
	_ B = x
)

const _ = 1
