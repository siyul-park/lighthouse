package visibility

// Public is used outside the package.
type Public struct {
	Name   string
	secret string
}

type private struct{ Value int }

// Exported is a function anyone may call.
func Exported() private { return private{} }

func unexported() {}

// Show is a method anyone may call.
func (p Public) Show() string { return p.secret }

func (p Public) hide() {}

const Limit = 1

const limit = 2
