package roles

import "testing"

type fixture struct{ name string }

const want = "hi ann"

var names = []string{"ann"}

func newFixture() fixture { return fixture{name: "ann"} }

func (f fixture) label() string { return f.name }

func TestGreet(t *testing.T) { check(t, newFixture().label()) }

func BenchmarkGreet(b *testing.B) { measure(b) }

func check(t *testing.T, name string) { assertGreeting(t, Greet(name)) }

func assertGreeting(tb testing.TB, got string) {
	tb.Helper()
	if got != want {
		tb.Fatal(got)
	}
}

func measure(b *testing.B) { _ = Greet(names[0]) }
