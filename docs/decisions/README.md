# Decision records

One file per decision, numbered in the order they were written:
`NNNN-short-name.md`. Numbers are never reused and never reserved by topic.

A record has four sections and a body of about twenty lines, plus the
measurements it rests on: **Context** (the forces, in the present tense),
**Decision**, **Rejected** (what else was on the table and why not),
**Consequences** (what the decision costs and what it obliges). A record is the
only place in `docs/` where a number may appear without a named constant behind
it: a measurement, with its date and method.

The status line at the top is one of `open` (not decided), `proposed` (decided,
not in the tree), `accepted` (in the tree), `superseded` (replaced — the line
names the record that replaced it). "Superseded by NNNN" is the only history a
record keeps; a record is never edited into a chronicle.
