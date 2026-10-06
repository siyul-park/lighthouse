package flow

type Tree struct {
	left, right *Tree
	value       int
}

func chain(a, b int) int {
	if a > 0 {
		return 1
	} else if b > 0 {
		return 2
	} else {
		return 3
	}
}

func labeled(n int) int {
	total := 0
outer:
	for i := 0; i < n; i++ {
		for j := 0; j < n; j++ {
			if i*j > 10 {
				break outer
			}
			total += j
		}
	}
	return total
}

func selects(a, b chan int, done <-chan struct{}) int {
	select {
	case v := <-a:
		return v
	case v := <-b:
		return -v
	case <-done:
		return 0
	default:
		return 1
	}
}

func fact(n int) int {
	if n <= 1 {
		return 1
	}
	return n * fact(n-1)
}

func (t *Tree) Sum() int {
	if t == nil {
		return 0
	}
	return t.value + t.left.Sum() + t.right.Sum()
}

func name(k int) string {
	switch k {
	case 0:
		return "zero"
	case 1, 2:
		return "few"
	default:
		return "many"
	}
}

func mixed(k int) string {
	switch k {
	case 0:
		return "zero"
	default:
		k++
		return "other"
	}
}

func logic(a, b, c, d bool) bool {
	if a && b && c || d {
		return true
	}
	return (a || b) && !c
}

func closures(xs []int) func() int {
	return func() int {
		n := 0
		for _, x := range xs {
			if x > 0 {
				n++
			}
		}
		return n
	}
}

func jump(n int) int {
	if n == 7 {
		goto done
	}
	n++
done:
	return n
}

func typed(v any) string {
	switch v.(type) {
	case int:
		return "int"
	case string:
		return "string"
	}
	return "other"
}

func wrap(a int, rest ...int) int { return inner(a, rest...) }

func reorder(a, b int) int { return inner(b, a) }

func inner(a int, rest ...int) int { return a + len(rest) }

func (t *Tree) left0() *Tree { return t.child() }

func (t *Tree) child() *Tree { return t.left }
