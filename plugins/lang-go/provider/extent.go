package provider

import (
	"go/ast"
	"go/token"
	"sort"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// extent is the whole of a declaration: its doc comment and the comment
// groups that lead it without a blank line between, through its last token and
// a comment that trails it on its last line. The comment map says which groups
// belong to the node.
func (u *unit) extent(node ast.Node) *sdk.Span {
	from, to := node.Pos(), node.End()
	groups := slicesOf(u.cmap[node])
	line := func(pos token.Pos) int { return u.fset.Position(pos).Line }
	for i := len(groups) - 1; i >= 0; i-- {
		g := groups[i]
		if g.End() <= from && line(g.End())+1 >= line(from) {
			from = g.Pos()
		}
	}
	for _, g := range groups {
		if g.Pos() >= to && line(g.Pos()) == line(to) {
			to = g.End()
		}
	}
	extent := span(u.fset, from, to)
	return &extent
}

// specExtent is the extent of one spec of a general declaration: the whole
// declaration when it is not parenthesized, the spec's own lines inside a
// group.
func (u *unit) specExtent(d *ast.GenDecl, spec ast.Node) *sdk.Span {
	if d.Lparen == token.NoPos {
		return u.extent(d)
	}
	return u.extent(spec)
}

func slicesOf(groups []*ast.CommentGroup) []*ast.CommentGroup {
	out := append([]*ast.CommentGroup(nil), groups...)
	sort.Slice(out, func(i, j int) bool { return out[i].Pos() < out[j].Pos() })
	return out
}
