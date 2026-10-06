package provider

import (
	"bytes"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// comments reports every comment group of the file. A group that ends on the
// line before a symbol starts is attached to that symbol.
func (b *builder) comments() {
	startingAt := map[int]string{}
	for _, s := range b.frag.Symbols {
		if _, taken := startingAt[s.Span.Start.Line]; !taken {
			startingAt[s.Span.Start.Line] = s.ID
		}
	}
	file := b.u.fset.File(b.u.file.Pos())
	if file == nil {
		return
	}
	for _, group := range b.u.file.Comments {
		from, to := file.Offset(group.Pos()), file.Offset(group.End())
		if from < 0 || to > len(b.u.src) || from >= to {
			continue
		}
		span := span(b.u.fset, group.Pos(), group.End())
		lineStart := bytes.LastIndexByte(b.u.src[:from], '\n') + 1
		attached := ""
		if len(bytes.TrimSpace(b.u.src[lineStart:from])) == 0 {
			attached = startingAt[span.End.Line+1]
		}
		b.frag.Comments = append(b.frag.Comments, sdk.Comment{
			Span:       span,
			Text:       strings.ReplaceAll(string(b.u.src[from:to]), "\r\n", "\n"),
			AttachedTo: attached,
		})
	}
}
