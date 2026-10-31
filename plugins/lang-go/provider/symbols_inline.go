package provider

import (
	"encoding/json"
	"go/ast"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// inlineBody is a function whose body is written between braces of one line:
// where the braces are and the statements the body holds, as written.
type inlineBody struct {
	// Line is the line of the declaration, where the function symbol is.
	Line int `json:"line"`
	// Open and Close are the positions just after `{` and of `}`: the text
	// between them is the body.
	Open       sdk.Position `json:"open"`
	Close      sdk.Position `json:"close"`
	Statements []string     `json:"statements"`
	// Plain is false when a comment sits in the body, which an expansion
	// would have to carry along.
	Plain bool `json:"plain"`
}

// inlineBodiesFact is the name of the file fact that lists the function
// declarations whose non-empty body sits on the line of its braces.
const inlineBodiesFact = "inline_bodies"

// inline records the function declaration when its body is on one line.
func (u *unit) inline(d *ast.FuncDecl) {
	body := d.Body
	if body == nil || len(body.List) == 0 {
		return
	}
	open, close := u.fset.Position(body.Lbrace), u.fset.Position(body.Rbrace)
	if open.Line != close.Line {
		return
	}
	found := inlineBody{
		Line:  u.fset.Position(d.Pos()).Line,
		Open:  position(u.fset, body.Lbrace+1),
		Close: position(u.fset, body.Rbrace),
		Plain: true,
	}
	for _, stmt := range body.List {
		from, to := u.fset.Position(stmt.Pos()).Offset, u.fset.Position(stmt.End()).Offset
		if from < 0 || to > len(u.src) || from >= to {
			return
		}
		found.Statements = append(found.Statements, string(u.src[from:to]))
	}
	for _, group := range u.file.Comments {
		if group.Pos() > body.Lbrace && group.End() < body.Rbrace {
			found.Plain = false
		}
	}
	u.inlined = append(u.inlined, found)
}

// publishInline sets the file fact once every function is recorded.
func (u *unit) publishInline() {
	if len(u.inlined) == 0 {
		return
	}
	data, err := json.Marshal(u.inlined)
	if err != nil {
		return
	}
	if u.frag.File.Facts == nil {
		u.frag.File.Facts = map[string]string{}
	}
	u.frag.File.Facts[inlineBodiesFact] = string(data)
}
