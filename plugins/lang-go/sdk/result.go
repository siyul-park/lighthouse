package sdk

import (
	"encoding/json"
	"sort"
)

// EncodedFragment is a fragment that is already JSON: a provider that kept the
// encoding of an earlier answer hands it over without decoding it.
type EncodedFragment struct {
	Path string
	JSON json.RawMessage
}

// MarshalJSON encodes the result with its encoded fragments in place among the
// others, in path order.
func (r IndexResult) MarshalJSON() ([]byte, error) {
	type wire struct {
		Fragments  []json.RawMessage `json:"fragments"`
		Notices    []string          `json:"notices"`
		Incomplete []Incomplete      `json:"incomplete"`
	}
	type entry struct {
		path string
		json json.RawMessage
	}
	entries := make([]entry, 0, len(r.Fragments)+len(r.Encoded))
	for i := range r.Fragments {
		data, err := json.Marshal(&r.Fragments[i])
		if err != nil {
			return nil, err
		}
		entries = append(entries, entry{r.Fragments[i].File.Path, data})
	}
	for _, e := range r.Encoded {
		entries = append(entries, entry{e.Path, e.JSON})
	}
	sort.SliceStable(entries, func(i, j int) bool { return entries[i].path < entries[j].path })
	out := wire{Notices: r.Notices, Incomplete: r.Incomplete}
	if r.Fragments != nil || len(r.Encoded) > 0 {
		out.Fragments = make([]json.RawMessage, len(entries))
		for i, e := range entries {
			out.Fragments[i] = e.json
		}
	}
	return json.Marshal(out)
}
