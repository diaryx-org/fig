```fig
title = Contributing
author = adammharris
created = 2026-07-04T16:43:09-06:00
updated = 2026-07-04T16:43:09-06:00
```

# Contributing to `fig`

Thank you for your interest in `fig`!

`fig` is hosted on GitHub. To contribute, use your GitHub account to submit a pull request. Ideally, these should be on their own branch, with an informative description about what the changes are and why they should be accepted.

There are good instructions on this topic [on the Github website](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/proposing-changes-to-your-work-with-pull-requests/creating-a-pull-request) if you don't already know how to do this.

## Commit messages

Subjects follow Conventional Commits — `type(scope): subject`. `fig` accepts
`add` and `feat` for additions, `change` and `refactor` for revisions, plus the
usual `fix`, `docs`, `test`, `ci`, `build`, `chore`. The scope is the surface
that moved (`toml`, `editor`, `c-api`, `cli`, `ts`, `rust`), and it is worth
setting: in a repo shipping four artifacts off one tree, it is the fastest
answer to "does this affect me". `.config/cliff.toml` groups
[CHANGELOG](CHANGELOG.md) by these, and anything it cannot parse lands in a
visible "triage before release" bucket rather than being dropped.

### `Behavioural-change:` trailers

If your change means a caller who upgrades **without editing a line of their own
code** would observe a difference, say so in a trailer at the end of the commit
message:

```
fix(editor): refuse value-replace on a TOML table header

<the body: why, and how it works>

Behavioural-change: `fig edit` at a TOML `[table]` path now refuses with
  `CannotReplaceTable`. It used to report success, having rewritten the
  header's NAME and left the table's entries where they were.
```

One trailer per observable difference; most commits carry none. Continuation
lines are indented two spaces. Write it for someone deciding whether to
upgrade — what used to happen, what happens now — not for a reviewer reading
the diff, which is what the body above it is for.

This matters more for `fig` than for most projects: `fig` is an editor, so its
contract is what bytes come out the other side of an edit. A refusal that used
to be a success, or a splice that lands somewhere new, moves no symbol and
breaks no build — it shows up as a wrong file on disk. The trailers are
collected into the changelog's **Behavioural changes** section; see
[CHANGELOG](CHANGELOG.md) for the full rule.

## Code of conduct

Code of conduct is simple for now: be nice! If you need more detail than that, try reading the one at [this link](https://github.com/stumpsyn/policies/blob/master/citizen_code_of_conduct.md). (You can replace `[COMMUNITY_NAME]` with "the `fig` community.")

Contact me at <amh421@icloud.com> if you have questions or any needs at all.