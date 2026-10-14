# 0001. Record every decision that shapes Meridian

- Status: accepted
- Date: 2026-10-14

## Context

Meridian is a whole operating system: bootloader, kernel, file system, desktop, a Windows personality and the tooling around them. Hundreds of choices will be made, and most of them only make sense together with the reasons and the alternatives that lost. Commit messages are too scattered for that, and a single DECISIONS file grows into a wall nobody reads.

## Decision

Every decision that changes how Meridian is built, what it depends on or how it behaves for users gets an architecture decision record in `docs/adr/`, numbered in order, written from `template.md`. A record is never edited to mean something else: a changed decision gets a new record, and the old one is marked "replaced by NNNN". `sextant adr new "title"` creates the next one.

Every outside file or piece of code Meridian uses needs its own record before it lands (see 0003).

## Alternatives considered

- **One DECISIONS.md.** Simple at first, unreadable at fifty entries, and changes rewrite history instead of adding to it.
- **Decisions in commit messages only.** Impossible to find later and impossible to link from the docs.

## Consequences

Anyone can read why Meridian is the way it is. Writing a record takes ten minutes per decision, which is the point: if a choice is not worth ten minutes of writing, it is probably not a real decision.
