package provider

import (
	"bytes"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// comments reports every comment group of the file. A group that ends on the
// line before a symbol starts is attached to that symbol.
func (u *unit) comments() {
	startingAt := map[int]string{}
	for _, s := range u.frag.Symbols {
		if _, taken := startingAt[s.Span.Start.Line]; !taken {
			startingAt[s.Span.Start.Line] = s.ID
		}
	}
	file := u.fset.File(u.file.Pos())
	if file == nil {
		return
	}
	for _, group := range u.file.Comments {
		from, to := file.Offset(group.Pos()), file.Offset(group.End())
		if from < 0 || to > len(u.src) || from >= to {
			continue
		}
		span := span(u.fset, group.Pos(), group.End())
		lineStart := bytes.LastIndexByte(u.src[:from], '\n') + 1
		attached := ""
		if len(bytes.TrimSpace(u.src[lineStart:from])) == 0 {
			attached = startingAt[span.End.Line+1]
		}
		u.frag.Comments = append(u.frag.Comments, sdk.Comment{
			Span:       span,
			Text:       strings.ReplaceAll(string(u.src[from:to]), "\r\n", "\n"),
			AttachedTo: attached,
		})
	}
}
