package steps

import (
	"errors"
	"flag"
	"fmt"
)

type Pair struct{ a, b int }

func Max[T int | float64](x, y T) T {
	if x > y {
		return x
	}
	return y
}

func helper(n int) int { return n }

// kinds has one step of every kind.
func kinds(xs []int, ch chan int) (total int, err error) {
	var unset int
	const limit = 10
	total = helper(xs[0]) + 2*limit
	total += xs[1] << 3
	total++
	if len(xs) == 0 {
		return 0, errors.New("empty")
	}
	if total > limit {
		total = -total
	}
	switch total {
	case 1:
		total = helper(1)
	}
	for _, x := range xs {
		total ^= x
	}
	{
		unset = int(total)
	}
	go helper(unset)
	defer fmt.Println("done")
	ch <- Max[int](1, 2)
	fmt.Sprint(Pair{1, 2}, []int{3}[0:1])
	func() { _ = helper(total) }()
outer:
	for {
		break outer
	}
	return total, nil
}

func jump(xs []int) {
	for range xs {
		if len(xs) > 1 {
			continue
		}
	}
}

type Foo struct{}

func (Foo) Run() {}

// Holder embeds one Foo and only has another.
type Holder struct {
	Foo
	Named Foo
	Foo2  Foo
}

// Plain has a field named as its type that embeds nothing.
type Plain struct {
	Foo Foo
}

type shape interface{ Area() int }

// names calls an error method, a local type, a type parameter, a package-level
// function variable and a method through a pointer.
func names[S shape](err error, s S) int {
	type local struct{ Pair }
	_ = err.Error()
	_ = (&local{}).sum()
	flag.Usage()
	return s.Area() + (&Pair{}).sum()
}

func (p *Pair) sum() int { return p.a + p.b }
