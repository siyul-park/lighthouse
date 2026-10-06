package testcases

import "testing"

func TestAddTable(t *testing.T) {
	cases := []struct{ a, b, want int }{{1, 2, 3}, {2, 3, 5}}
	for _, tc := range cases {
		t.Run("case", func(t *testing.T) {
			if Add(tc.a, tc.b) != tc.want {
				t.Fatal("add")
			}
		})
	}
}

func TestNested(t *testing.T) {
	t.Run("outer", func(t *testing.T) {
		t.Run("inner", func(t *testing.T) { _ = Double(1) })
	})
}

func TestPlain(t *testing.T) { helper(t) }

func Testing() {}

func helper(t *testing.T) { _ = Add(1, 1) }

func BenchmarkAdd(b *testing.B) {
	for i := 0; i < b.N; i++ {
		Add(i, i)
	}
}

func ExampleDouble() { _ = Double(2) }
