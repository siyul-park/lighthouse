package aliases

// Store keeps values.
type Store struct{ data map[string]int }

// Repo is another name for Store.
type Repo = Store

// Handler is a callback shape.
type Handler = func(string) error

// Reader reads values.
type Reader = interface{ Read() int }

// Read returns the value count; its receiver is spelled through the alias.
func (r *Repo) Read() int { return len(r.data) }

func use(h Handler) error { return h("x") }

func count(r Repo) int { return r.Read() }
