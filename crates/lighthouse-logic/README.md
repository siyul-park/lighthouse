# lighthouse-logic

The analysis of a decision set: what the decisions say, read as logic. Pure:
it never runs a check, an engine or a plugin, and does not depend on
`lighthouse-checks`.

- `translate`: the CEL `where` of a decision (variables inlined) as a union of
  conjunctive queries with comparisons (`Ucq`), or the first construct that
  leaves the fragment (`Reject`).
- `Ucq::holds`: evaluates the query over the fact value of a subject, the same
  JSON the checks read.

Later steps add the tiers, containment and the label store (see the LG-3 spec).
