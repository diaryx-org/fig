---
title: fig
nav_title: fig
nav_order: 20
description: 'fig — lossless parsing and editing of config files. JSON, JSON5, YAML, TOML, ZON: set a value and get a one-line diff, with every comment, key order, and blank line preserved.'
audience: public
part_of: '[fig](/README.md)'
id: gns15jg
---
<section class="pj-head">
  <div class="wrap">
    <p><a class="crumb" href="../about/#projects">diaryx.org / projects /</a></p>
    <div class="pj-title" style="margin-top: 1rem">
      <h1>fig</h1>
      <span class="pj-tags">
        <span class="tag-chip">Zig</span>
        <span class="tag-chip">MIT / Apache-2.0</span>
      </span>
    </div>
    <p class="pj-tagline">
      Lossless parsing and editing of config files — down to every
      comment, key order, and blank line.
    </p>
  </div>
</section>

<section class="pj-main">
<div class="wrap pj-layout reveal">
<div class="pj-body">

Editing config files programmatically shouldn't mean reformatting
them. fig parses JSON, JSON5, YAML, TOML, ZON, and its own dialect
into a lossless AST, then splices your change in: a one-line diff,
everything else byte-for-byte identical.

- **Set a value, keep the file.** `fig set config.yaml service.replicas 5` touches one line.
- **Comments are data.** Attach one inline: `fig comment --inline config.yaml service.replicas "bumped"` — and they survive every edit and conversion.
- **Convert without loss.** YAML → JSON5 → TOML carries comments along.
- **Edit embedded config too.** Frontmatter inside Markdown is just another document to fig.

## Where it fits

fig is the metadata layer of [Diaryx](id:org/80k72t9) —
frontmatter in every entry goes through it, and
[flower](id:flower/v817zvg) is a structural editor built on
its tree. Standalone, it's a library (crates.io, npm) and a CLI
for anyone who scripts against config files.

## Status

Actively maintained and published: the Rust crate wraps the Zig
core, an npm package ships it for JavaScript, and Homebrew carries
the CLI.

</div>
<aside class="pj-aside">
<div class="install">
<span class="install-head">Install</span>
<div class="cmd">brew install diaryx-org/tap/fig <small>CLI</small></div>
<div class="cmd">cargo add fig <small>Rust</small></div>
<div class="cmd">npm install @diaryx/fig <small>JavaScript</small></div>
</div>
<div class="facts">
<div class="row"><span class="k">Language</span><span class="v">Zig (Rust bindings)</span></div>
<div class="row"><span class="k">Used by</span><span class="v"><a href="../prov/index.md">prov</a> · <a href="../flower/index.md">flower</a> · <a href="../index.html">Diaryx</a></span></div>
<div class="row"><span class="k">Source</span><span class="v"><a href="https://github.com/diaryx-org/fig">github.com/diaryx-org/fig</a></span></div>
<div class="row"><span class="k">Packages</span><span class="v"><a href="https://crates.io/crates/fig">crates.io/crates/fig</a> · <a href="https://www.npmjs.com/package/@diaryx/fig">npmjs.com/@diaryx/fig</a></span></div>
<div class="row"><span class="k">License</span><span class="v">MIT or Apache-2.0</span></div>
</div>
</aside>
</div>
</section>
