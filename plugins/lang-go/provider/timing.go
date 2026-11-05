package provider

import (
	"sync"
	"time"
)

// timings adds up how long each phase of one request took, in the order the
// phases first ran. A phase of a loop is timed once for the whole of it.
type timings struct {
	mu    sync.Mutex
	order []string
	spent map[string]time.Duration
}

type phase struct {
	name  string
	spent time.Duration
}

// Phase names of the breakdown an index request reports to the host, which
// prints it under `--timings`.
const (
	// phasePlan is reading the project's directories for the `api` of the
	// units asked for and the interfaces that are loaded with them.
	phasePlan = "plan"
	// phaseLoad is the time the analysis waited for the go command to list,
	// compile and type-check packages.
	phaseLoad = "load"
	// phaseAnalyse is reading the code model out of the loaded packages.
	phaseAnalyse = "analyse"
	// phaseSerialise is gathering the fragments of the answer.
	phaseSerialise = "serialise"
)

// measure starts timing phase; the returned function stops it.
func (t *timings) measure(phase string) func() {
	started := time.Now()
	return func() { t.add(phase, time.Since(started)) }
}

func (t *timings) add(phase string, d time.Duration) {
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.spent == nil {
		t.spent = map[string]time.Duration{}
	}
	if _, seen := t.spent[phase]; !seen {
		t.order = append(t.order, phase)
	}
	t.spent[phase] += d
}

// phases are the phases timed, in order.
func (t *timings) phases() []phase {
	t.mu.Lock()
	defer t.mu.Unlock()
	out := make([]phase, 0, len(t.order))
	for _, name := range t.order {
		out = append(out, phase{name, t.spent[name]})
	}
	return out
}
