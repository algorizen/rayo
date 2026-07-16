# RFC-NNNN: Title

- **Status:** Draft | Accepted | Rejected | Postponed
- **Shepherd:** —
- **Tracking issue:** —
- **Disposition reasoning:** *(filled by shepherd at close)*

## Summary

One paragraph. What changes, for whom.

## Motivation

What problem does this solve? Who hits it today, and how badly? Evidence beats assertion — link issues, code, numbers.

## Design

The complete proposal. Include:

- Public API (signatures, examples of user code before/after)
- Behavior at the edges (errors, cancellation, both GIL and free-threaded builds)
- Interaction with architecture invariants (ARCHITECTURE.md §6) — state explicitly which are touched and how the design respects them
- Wire/serialization format changes, if any

## Alternatives considered

What else could solve this, and why is the proposal better? "Do nothing" is always an alternative — cost it.

## Drawbacks and risks

What gets worse? Complexity, performance, compatibility, maintenance burden. Be the proposal's best critic here — the shepherd will be otherwise.

## Migration and compatibility

Effect on existing user code, on the conformance scoreboard, and on the docs. Deprecation path if applicable.

## Open questions

What should be resolved during the comment period vs deferred to implementation?
