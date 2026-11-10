---
name: decimate-comments
description: Cut comments back to the essential why. Use when asked for a comment pass, a comment-slashing pass, to decimate or slash comments, or to cut comments down.
---

# Decimate comments

Cut down on comments severely, keeping only the most essential *why* information in each
comment. A comment is not longer than the function it describes. Remove information about what
things used to do or used to be. Write in simple, straightforward style, without metaphor or
simile, for a human.

`CLAUDE.md`'s **Comments** section is the standing rule; this is the pass that enforces it.

## Spelling

Use American spelling, always — in the comments you keep and the comments you rewrite. British
spelling is a merge-blocking defect. Fix any you find in the files in scope: `behavior` not
`behaviour`, `color` not `colour`, `recognize` not `recognise`, `canceled` / `signaled` /
`traveled` not `cancelled` / `signalled` / `travelled`, `center` not `centre`, `analyze` not
`analyse`, `license` (noun and verb) not `licence`.

Check before finishing:

```bash
grep -rniE 'behaviour|colour|favour|honour|labour|neighbour|recognis|organis|initialis|serialis|normalis|minimis|maximis|prioritis|summaris|realis|apologis|analys(e|ing)|centre|metre|licence|defence|offence|cancell(ed|ing)|signall(ed|ing)|travell(ed|ing)|modell(ed|ing)|labell(ed|ing)|fulfil[^l]|judgement|whilst|amongst' <files>
```

Read each hit: `cancellation` and `analysis` are American and stay.

## Delete

- Restatements of the code, in any dress: `/// Returns the name.`, a doc that lists the
  parameters the signature already names.
- History. "used to be", "the first version", "the bug was", "this used to race". The log
  remembers, and a reader with the current code in front of them cannot use any of it.
- Argument for a decision the code already makes — the same point in three sentences, then
  again in the test's doc.
- Metaphor, simile, and bold used for emphasis rather than for a warning.
- Design-document prose. Link to the document instead.
- The phrases "surface" (as a verb), "quietly", and "flag rather than bury".

## Keep

- **Why**, where the why is not derivable: a constraint, a discarded alternative, a bug this
  shape prevents.
- Measured numbers and where they came from — the threshold someone found by looking.
- Units, ranges and frames the type does not carry.
- Numerical hazards: cancellation, saturation, a tolerance and its reason.
- Invariants a caller must uphold.
- Module docs. They carry the shared context so the items below need not repeat it.
- A test's doc, cut to one or two sentences saying what breaks.

**Do not delete a comment you do not understand.** It is the one most likely to be load-bearing.
Read the code until it is either derivable — then cut it — or not.

## Writing style

Comments should be written in a straightforward style without clever grammar or too much
nesting. Be specific where possible, and avoid metaphor and simile. The following table gives
examples of writing tics that make comments hard to understand, as well as examples with
corrections.

| Tic | Example | Better |
| --- | --- | --- |
| Negation-first reveal | It is not a wrong answer. It is a non-answer. | It does not answer the question. |
| Significance designation | It is the leg that answers the actual question. | This leg compares the two configurations on identical hardware, so it isolates the effect we’re testing. |
| Abstraction agency | Median hides it. / The table shows it. | The median stays flat, so the spike appears only in the p99 values. / In Table 2, rows 3–5 run about twice as slow as the rest. |
| Deferred noun | Five of those six rows are one cluster. The sixth is not. | Five of the six rows come from cluster A, but the sixth comes from cluster B. |
| Coy or thesis-shaped header | What "exhausted" means / The one gap that does clear the bar | Definition of "exhausted" / Memory bandwidth gap (above threshold) |
| Aphoristic closer | It is the kind of number that looks like evidence and is not. | This figure comes from a single run with no variance estimate, so it can’t support the conclusion. |
| Straw-man knockdown | "It thinks twice as long" is the obvious reading, and it is wrong. | The output is twice as long, but most of the extra tokens restate the prompt. |
| Fragment cadence | Six legs. One GPU, one server build, one sampler, one question set. | All six legs ran on the same GPU, server build, sampler, and question set. |
| Dressed metaphor | That is information loss wearing the costume of a style fix. | The change was labeled a style fix, but it removes the units column, so readers lose information. |

## Scope

State which files before starting, and leave the rest. Two things are out of bounds unless
asked: code moved verbatim from elsewhere, where the comments traveled with it and the
verbatim move is the property worth keeping, and anything vendored.

Prefer making the code say it — a named constant, a smaller function, a better type removes the
comment that explained it. That is a code change: say so rather than slipping it in.

## Check

Count before and after; the count is the review artifact.

```bash
grep -rc '^\s*//' <files>
```

Work in batches with `cargo test` between them — a bulk edit that takes a line of code with it
is otherwise found much later. The diff should be nearly all deletions.
