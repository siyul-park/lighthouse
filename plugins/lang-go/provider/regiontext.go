package provider

import (
	"sort"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// regionText is the text a fence is compiled as, one entry per byte telling the
// host bytes it came from.
type regionText struct {
	text []byte
	host [][2]int
}

// remap puts the spans of the fragment of a fence's file where the host file
// shows them.
type remap struct {
	text  regionText
	lines []int
	host  []int
	// module is the module the fence declares its symbols in, and shared
	// says that it is one of the project's: the imports of the fence are then
	// no imports of the module.
	module string
	shared bool
}

// unmapped marks the bytes of a fence's file that the host does not show: the
// lines that wrap the fence.
const unmapped = -1

// around is the text between head and tail, which the host does not show.
func (t regionText) around(head, tail string) regionText {
	text := append([]byte(head), t.text...)
	text = append(text, tail...)
	host := make([][2]int, 0, len(text))
	for range head {
		host = append(host, [2]int{unmapped, unmapped})
	}
	host = append(host, t.host...)
	for range tail {
		host = append(host, [2]int{unmapped, unmapped})
	}
	return regionText{text: text, host: host}
}

// hostRange is the host bytes of the bytes start..end of the text: from where
// the first starts to where the last the host shows ends. It is false when the
// host shows none of them.
func (t regionText) hostRange(start, end int) ([2]int, bool) {
	if start < 0 || start >= len(t.host) || t.host[start][0] == unmapped {
		return [2]int{}, false
	}
	from := t.host[start]
	for at := min(max(end, start+1), len(t.host)) - 1; at >= start; at-- {
		if t.host[at][0] != unmapped {
			return [2]int{from[0], max(t.host[at][1], from[0])}, true
		}
	}
	return [2]int{}, false
}

// fragment is the fragment as the host file shows it, with the file path as
// its own. A symbol the host does not show (the wrapper function) is dropped
// with what is about it: what it declared or called is the module's, and a
// call, a use or a declaration keeps its place only where the host shows it.
func (m remap) fragment(f *sdk.Fragment, path string) *sdk.Fragment {
	symbols, dropped := m.symbols(f.Symbols, path)
	tests := make([]sdk.TestCase, 0, len(f.Tests))
	for _, test := range f.Tests {
		if !dropped[test.Symbol] {
			tests = append(tests, test)
		}
	}
	return &sdk.Fragment{
		File:      sdk.FileInfo{Path: path},
		Modules:   f.Modules,
		Symbols:   symbols,
		Edges:     m.edges(f.Edges, dropped),
		Functions: m.functions(f.Functions, dropped),
		Tests:     tests,
		Comments:  m.comments(f.Comments, dropped),
	}
}

// comments are the comment groups the host shows, at their place. The text is
// the fence's: what the host shows without the indentation of a list item.
func (m remap) comments(all []sdk.Comment, dropped map[string]bool) []sdk.Comment {
	shown := make([]sdk.Comment, 0, len(all))
	for _, comment := range all {
		span, ok := m.span(comment.Span)
		if !ok {
			continue
		}
		comment.Span = span
		if dropped[comment.AttachedTo] {
			comment.AttachedTo = ""
		}
		shown = append(shown, comment)
	}
	return shown
}

// symbols are the symbols the host shows, at their place, and the ids of the
// ones it does not.
func (m remap) symbols(all []sdk.Symbol, path string) ([]sdk.Symbol, map[string]bool) {
	dropped := map[string]bool{}
	shown := make([]sdk.Symbol, 0, len(all))
	for _, symbol := range all {
		span, ok := m.span(symbol.Span)
		if !ok {
			dropped[symbol.ID] = true
			continue
		}
		symbol.Span, symbol.File = span, path
		if symbol.Extent != nil {
			extent, ok := m.span(*symbol.Extent)
			symbol.Extent = nil
			if ok {
				symbol.Extent = &extent
			}
		}
		shown = append(shown, symbol)
	}
	for i := range shown {
		if dropped[shown[i].Owner] {
			shown[i].Owner = ""
		}
	}
	return shown, dropped
}

func (m remap) functions(all []sdk.FunctionSummary, dropped map[string]bool) []sdk.FunctionSummary {
	shown := make([]sdk.FunctionSummary, 0, len(all))
	for _, function := range all {
		if dropped[function.Symbol] {
			continue
		}
		events := function.Events[:0:0]
		for _, event := range function.Events {
			if span, ok := m.span(event.Span); ok {
				event.Span = span
				events = append(events, event)
			}
		}
		function.Events = events
		shown = append(shown, function)
	}
	return shown
}

func (m remap) edges(all []sdk.Edge, dropped map[string]bool) []sdk.Edge {
	shown := make([]sdk.Edge, 0, len(all))
	for _, edge := range all {
		if dropped[edge.To] || (m.shared && edge.Kind == edgeImports && edge.From.Symbol == "") {
			continue
		}
		if dropped[edge.From.Symbol] {
			edge.From = sdk.Node{Module: m.module}
		}
		if edge.Site != nil {
			site, ok := m.span(*edge.Site)
			edge.Site = nil
			if ok {
				edge.Site = &site
			}
		}
		shown = append(shown, edge)
	}
	return shown
}

// span is the host span of a span of the fence's file; it is false where the
// host shows nothing of it.
func (m remap) span(s sdk.Span) (sdk.Span, bool) {
	from, ok := m.offset(s.Start)
	if !ok {
		return sdk.Span{}, false
	}
	to, ok := m.offset(s.End)
	if !ok {
		return sdk.Span{}, false
	}
	at, ok := m.text.hostRange(from, to)
	if !ok {
		return sdk.Span{}, false
	}
	return sdk.Span{Start: positionAt(m.host, at[0]), End: positionAt(m.host, at[1])}, true
}

func (m remap) offset(p sdk.Position) (int, bool) {
	if p.Line < 1 || p.Line > len(m.lines) || p.Col < 1 {
		return 0, false
	}
	return m.lines[p.Line-1] + p.Col - 1, true
}

// newRegionText is the fence as it is written, each byte at its place in the
// host: its own bytes where the text is copied as is, the whole segment where
// it was changed on the way (a tab, a line ending), the whole fence without a
// map.
func newRegionText(source *sdk.EmbeddedSource) regionText {
	from, to := int(source.SourceRange[0]), int(source.SourceRange[1])
	host := make([][2]int, len(source.Text))
	for i := range host {
		host[i] = [2]int{from, to}
	}
	if source.TextMap == nil && to-from == len(source.Text) {
		for i := range host {
			host[i] = [2]int{from + i, from + i + 1}
		}
	}
	for _, segment := range source.TextMap {
		textFrom, textTo := int(segment.TextRange[0]), int(segment.TextRange[1])
		hostFrom, hostTo := int(segment.SourceRange[0]), int(segment.SourceRange[1])
		linear := textTo-textFrom == hostTo-hostFrom
		for i := textFrom; i < textTo && i < len(host); i++ {
			if linear {
				host[i] = [2]int{hostFrom + i - textFrom, hostFrom + i - textFrom + 1}
			} else {
				host[i] = [2]int{hostFrom, hostTo}
			}
		}
	}
	return regionText{text: []byte(source.Text), host: host}
}

// lineStarts are the offsets at which the lines of text begin.
func lineStarts(text []byte) []int {
	starts := []int{0}
	for i, b := range text {
		if b == '\n' {
			starts = append(starts, i+1)
		}
	}
	return starts
}

// positionAt is the position of a byte offset, given the starts of the lines.
func positionAt(starts []int, offset int) sdk.Position {
	line := sort.Search(len(starts), func(i int) bool { return starts[i] > offset })
	return sdk.Position{Line: line, Col: offset - starts[line-1] + 1}
}
