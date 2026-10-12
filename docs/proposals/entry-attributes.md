```fig
title = Entry attributes
description = A keyed list of attributes on an entry, between its key and its value — what iCalendar, vCard, MIME headers and XML data vocabularies need — and the rule that keeps fig a data tree while it takes them
author = adammharris
created = 2026-10-12
status = draft
part_of = [Proposals](/docs/proposals/proposals.md)
```

# Entry attributes

> **Status: DRAFT.** Written against `main` at ef72fab. Nothing here is
> built. The reference for what lossless editing of the motivating
> formats means is two Node projects outside the org, `~/Code/ical` and
> `~/Code/vcard`. Each has its own content-line parser and editor, with 41
> tests between them. If fig takes these formats, those tests are what it
> has to pass.

## 1. The claim

An entry should be able to carry **attributes**: a keyed list of named
values, written between its key and its value, that qualify the value
without being part of it.

```
DTSTART;TZID=Europe/London:20261012T090000
Content-Type: text/html; charset=utf-8
<dtstart><parameters><tzid><text>Europe/London</text></tzid></parameters>…
```

Each line is one entry: a key (`DTSTART`, `Content-Type`), a value, and
attributes that say how to read the value (`TZID`, `charset`). fig has no
place for the middle part. A language can drop it, but then fig is not
lossless. It can fold it into the key, but then `DTSTART` is not a path.
Or it can fold it into the value, so that `get` returns
`TZID=Europe/London:2026…`. Each of these is wrong in its own way.

## 2. Where the line is, and why attributes do not cross it

Attributes are what twig's tree has and fig's lacks, so it is fair to ask
whether this makes fig twig, and whether the two should merge. They should
not, and the reason gives the rule for this proposal.

**A project is a set of formats that convert into each other without an
invented mapping.**

- JSON → TOML, YAML → JSON and Markdown → Djot are uncontested. A value
  stays the same value, and a paragraph stays a paragraph.
- JSON → Markdown is contested. Is a mapping a heading or a definition
  list? Is a string a paragraph? Somebody has to decide. That decision is
  not fig's or twig's to make, and the boundary between the projects is
  where it would have to be made.

The difference underneath is **mixed content**, meaning text with
structure woven through it (`Some *bold* text`). Twig's fifty node kinds,
its block and inline levels, and its rendering all exist for mixed
content. A fig value is a scalar or a container and never a sentence with
a node inside it.

Attributes do not bring mixed content. An attribute is a keyed scalar on
an entry, and a property with attributes is still data. iCalendar
converts to JSON without anyone having to decide anything: RFC 7265
(jCal) defines the mapping. It converts to Markdown no better than JSON
does. By the rule, it belongs in fig.

## 3. Who needs it

| format | entry | attributes |
|---|---|---|
| iCalendar (RFC 5545) | `ATTENDEE;CN=Mary;ROLE=CHAIR:mailto:m@x` | `CN`, `ROLE` |
| vCard 3.0/4.0 | `EMAIL;TYPE=work,pref:m@x` | `TYPE` (multi-valued) |
| vCard 2.1 | `TEL;WORK;VOICE:+44…` | bare: a value with no name |
| MIME and HTTP headers | `Content-Type: text/html; charset=utf-8` | `charset` |
| xCal, xCard (RFC 6321, 6351) | `<dtstart><parameters>…` | the `<parameters>` element |
| Debian control, `.desktop` | `Name[de]=Datei` | the locale |

The last row is a weaker case. A locale suffix could equally be modelled
as part of the key. The others cannot. The content-line formats are the
strongest case, because they are where the user-facing value is: diaryx
already imports GEDCOM, and calendars and contacts are the other two
formats a family archive is made of.

## 4. The shape

### 4.1 In the node table

Attributes go in a **side table**, beside `regions`, `mentions` and the
anchor and tag tables, and not in a new row kind:

```js
t.attribute(entryRow, nameSpan, valueSpans, { bare: false })
```

- Each attribute has a name with its span, and one or more values, each
  with its span. `TYPE=work,pref` is one attribute with two values.
- A bare attribute (vCard 2.1's `WORK`) has a value and no name. The
  language says what name it stands for (`TYPE`).
- Order is kept. Repeated names are kept, since vCard 3.0 writes
  `type=INTERNET;type=HOME`. A read merges them.

A side table keeps the eight node kinds closed, and every consumer that
does not ask about attributes is unaffected. It is how twig holds
attributes too (`AST.attrs`, indexed by `Node.attrs`), which is evidence
that it holds up across about fifty kinds and six parsers.

### 4.2 Syntax and renderers

An edit has to spell an attribute. `Syntax` gains:

- `attr_open`: the byte that opens the attribute list (`;` for content
  lines, `; ` for MIME)
- `attr_kv`: what joins a name to its values (`=`)
- `attr_value_sep`: what joins one attribute's values (`,`)

Quoting is the language's business. A content-line value holding `:`,
`;` or `,` is double-quoted, and a quote inside one is RFC 6868's `^'`.
So `attribute` joins `value`, `entry`, `item`, `tail` and `key` as a
fragment the language may render. XML vocabularies, where an attribute is
an element, always render it.

### 4.3 Paths and edits

A path names an attribute with `;` after the entry, the content lines'
own spelling:

```sh
fig get cal.ics 'VEVENT[0].DTSTART;TZID'
fig set cal.ics 'VEVENT[0].DTSTART;TZID' America/New_York
fig delete card.vcf 'VCARD[0].EMAIL[0];TYPE'
```

`;` is free in fig's path grammar today (`args.parsePath` reads `.`,
`[` and quoted keys). An attribute ends a path. The editor gains
`setAttribute(path, name, values)` and `deleteAttribute(path, name)`.
Both are splices in the span between the key and the value, under the
reparse net. Replacing an attribute keeps its spelling: Apple writes
`type=`, and a replaced value is still `type=`.

## 5. Conversion

fig's rule for what a target cannot hold is the one attributes follow.
Today, converting YAML's `null` to TOML drops the value, `diagnose`
reports `ValueDropped`, and `{ lossless: true }` carries it through in a
`$fig` envelope instead. So:

- **By default, an attribute the target cannot hold is dropped and
  reported.** A new warning, `AttributeDropped`, comes with a path, so
  `diagnose(Format.Toml)` on a calendar names every `TZID` it would lose.
- **`lossless: true` carries attributes in the `$fig` envelope**, as it
  does every other value. The envelope is fig's own reversible spelling,
  opted into. It is not a claim about what the attribute means.
- **jCal and jCard are dialects, not the default.** Their array-per-property
  shape (`["dtstart", {"tzid": "…"}, "date-time", "…"]`) is a mapping a
  standard made, so fig may offer it by name. But turning every
  conversion of a calendar into arrays would be fig deciding something the
  rule in §2 says it does not decide.

## 6. XML

With attributes, **XML vocabularies become fig formats, but generic XML
does not.**

Generic XML → JSON is the textbook contested conversion: BadgerFish,
Parker, JsonML and xml2js each decide differently. The questions they
answer are:

- whether an attribute and a child element are told apart
- whether a repeated element becomes an array, and what happens when it
  repeats only sometimes
- where an element's text goes when it also has children
- whether the order of differently named siblings matters

Those decisions are exactly what §2 keeps out of fig. Generic XML,
including XHTML, DocBook and SVG with its `<text>`, is mixed content or
close to it, and stays twig's.

A **vocabulary that defines its own data mapping** passes the test,
because the vocabulary has made the decisions:

- plist is one already. Apple defines it (a `dict`, `key` and value
  elements, no attributes), and fig reads it today.
- xCal and xCard are the next: RFC 6321 and RFC 6351 define them as the
  same data as iCalendar and vCard. With attributes, an `.ics` and its xCal
  twin parse to the same tree, and the conversion between them is as
  uncontested as JSON → YAML.

So fig and twig both read XML, but never the same XML for the same
purpose. A runtime format never joins content sniffing, so the choice is
by extension or by name, as it already is for plist.

## 7. What else the content-line formats need

Attributes are the blocker. Two smaller things are needed beside them:

- **Values made of parts.** `N:Smith;Mary;;;` and `CATEGORIES:a,b`
  are lists inside a value, with no brackets. This landed in ef72fab: a
  sequence row names its own `item_sep` and can be `open`. A structured
  value is an open sequence with `item_sep = ";"`.
- **Repeated names, addressed by name.** A component holds several
  `ATTENDEE`s, interleaved with other properties, and a file holds many
  `VEVENT`s. As a mapping with `duplicates: "keep"`, the later ones exist
  but no path reaches them. The `ical` and `vcard` projects address them
  as `ATTENDEE[1]` (the second entry named `ATTENDEE`) and
  `VEVENT[UID=x]` (the event whose `UID` is `x`). Whether fig takes either
  spelling is the open question this proposal does not settle. It may
  deserve its own proposal, since INI files and Java properties with
  repeated keys would use it too.

## 8. How to find out

1. Write the content-line grammar as a runtime language with
   `@diaryx/fig/kit`. Keep the attributes the language cannot yet report in
   a test-only side channel, and run `ical`'s and `vcard`'s tests through
   it. That shows how far fig already goes and gives §4 a measured list.
2. Add the side table and the `Syntax` fields to core, carry them on the
   wire, and turn the side channel into `t.attribute`.
3. Serve the language as a CLI helper (`languages.figl`). Then `fig get`,
   `fig set`, `fig convert` and `fig mcp` work on `.ics` and `.vcf`, and
   `ical` and `vcard` keep only what fig cannot know: `list`, `add`,
   `move`, `cancel`, and each version's required properties.

## 9. Open

- **What a read merges.** Should `EMAIL;type=INTERNET;type=HOME` read as
  one `TYPE` with two values (what both projects do) or two attributes? A
  merged read is what a caller wants, but a write has to know which of
  the two it is replacing.
- **Bare attributes outside vCard 2.1.** HTML's `disabled` is one, but
  HTML is twig's. If 2.1 is the only fig format that has them, `bare`
  might be better as a language-side spelling than a table column.
- **Attributes on a container.** A component has none in iCalendar, but
  an XML vocabulary's element might. The table above allows it, and
  nothing in §3 needs it yet.
