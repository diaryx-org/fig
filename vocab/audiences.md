---
title: Audiences
part_of: '[fig](/README.md)'
contents:
- '[Public](/vocab/public.md)'
---

# Audiences

Who a document in this repository may be published to. The `audience:` field
is closed against this list (`fields.audience` in the prov config), so a value
that is not a term below is a `prov check` finding rather than a document that
quietly stops publishing.

The vocabulary is *reified*: each term is a document, so it has a body to say
who the readership is and somewhere to hang the settings a site rendered for
them is built with. `plates` reads the front page off the term node.

Nothing in this directory declares an `audience:` of its own, so none of it is
published.
