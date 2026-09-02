# 0001 — Documentation and comments

Status: accepted (2026-09-02)

## Context

Knowledge lives in a long Russian README and in comments that are half of some
files; they carry old measurements, promises about future phases and claims of
impossibility, and a review found several of each false. One developer with an
AI assistant cannot tell a stale number from a true one by looking.

## Decision

`docs/` is the system documentation, in English, describing the tree as it is:
files by path, never by line; restrictions once, on a limitations page, with
the cause in the present tense. Plans stay out of `docs/`; `docs/roadmap.md`
is the one dated exception, a Russian plan deleted when done. A number lives in
a named constant or is an external fact with a source, and prose names the
constant; a measurement appears only in a decision record with date and
method; a wrong number is never replaced by another number. "Cannot be done"
is written only next to the test that shows it. Verbatim duplicates of a
comment are merged into one owner on sight. Comments stay Russian: `//!` at
most ten lines (what the file owns, which invariant, where checked, which
page), `///` the contract, `//` non-obvious mechanics; no counterfactual longer
than a sentence, no self-defence, no line numbers, no "same as its neighbour"
instead of a shared constant — `.proto` included. Nothing is translated in
place: an English page with a line budget replaces the Russian source in the
same commit. `buildgen/tests/test_docs.py` holds the mechanical part.

## Rejected

Translating README as it is; a documentation generator or length metrics;
identifier checks by regular expression; a separate pass over the long
comments — they sit in the files the reading model rewrites.

## Consequences

Two languages in the tree for months. A page is written in the commit that
touches its subsystem, never in a documentation week. The roadmap's line
numbers are tolerated because that file is dated and temporary.
