```fig
title = Fig's 2nd Epoch: Texas Everbearing
part_of = [Devlogs](/docs/devlogs/devlogs.md)
author = adammharris
date = 2026-09-14
contents
> * [Texas Everbearing The Rules](/docs/devlogs/2-texas-everbearing/texas-everbearing-the-rules.svg.yaml)
> * [Texas Everbearing Same Format Three Times](/docs/devlogs/2-texas-everbearing/texas-everbearing-same-format-three-times.svg.yaml)
> * [Texas Everbearing Helper Latency](/docs/devlogs/2-texas-everbearing/texas-everbearing-helper-latency.svg.yaml)
> * [Texas Everbearing Hooks Retired](/docs/devlogs/2-texas-everbearing/texas-everbearing-hooks-retired.svg.yaml)
```

It is time for Fig's new epoch:
**Texas Everbearing**!

> a versatile and hardy variety known for its consistent production of sweet, flavorful fruit
> and its adaptability to various growing conditions.
>
> — [PlantMeGreen.com](https://plantmegreen.com/products/fig-everbearing)

The goal of this epoch is, as its name says, to be "Everbearing."
To allow Fig to read, print, and edit *any* possible data representation format, in the past or in the future!
The only way to do this was to introduce the concept of a *runtime* language.

Those who are familiar with Zig know that its `comptime` is the star of the show. Because of this, it is relatively easy to write support for a new language as a few new Zig files. But I can't compile *everything* into one binary. There are an unlimited number of structured data languages out there. Am I doomed to keep adding one language after another, until we have them all?

No! Thankfully, I can simply teach Fig the *rules* of a language, and allow anyone fulfilling these rules to count as a language.

Tools that turn many readers into the same AST already exist. These mirror the "describe" and "read" rules that Fig has. Conversion tools also implement the "write" side on a whole-file basis. But to allow minimal, lossless editing, we needed a new verb: "render." It is similar to "write," but for specific values and spellings rather than a whole-file basis.

![Any format that follows the rules is a language](/docs/devlogs/2-texas-everbearing/texas-everbearing-the-rules.svg)

Another goal I wanted to accomplish was to keep Fig at zero dependencies. I wanted Fig to be the very fastest and most lightweight of all structured data libraries. So, rather than make Fig itself depend on a runtime library, I introduced a JSON protocol that works for any arbitrary runtime layer. And to prove it worked across multiple runtimes, this Fig epoch I created both [`fig-lua`](https://github.com/diaryx-org/fig-lua) and [`fig-quickjs`](https://github.com/diaryx-org/fig-quickjs), to prove that I could write a language in either Lua or JavaScript, and it would work the same way.

![Every format fig ships, written again as a script](/docs/devlogs/2-texas-everbearing/texas-everbearing-same-format-three-times.svg)

On average, the amount of lines of code is about half of what it would take in Zig. Going forward, JavaScript will likely be the primary extension language due to its ubiquity.

Both the Lua and QuickJS implementations are written in Rust and consume Fig's Rust bindings and `mlua` and `rquickjs`, respectively. In Cargo projects, the fig dependency is deduplicated, so you get just one `fig` and one runtime dependency—only what you need. On the web, you can take advantage of the environment's existing support for JavaScript.

When it comes to performance, a runtime format can never beat a compiled in format. But it can get quite close—about 6 milliseconds longer.

![What a script format costs per command](/docs/devlogs/2-texas-everbearing/texas-everbearing-helper-latency.svg)

What we don't have yet: JIT-enabled runtimes, and packaging the helper functions as libraries. But those are packaging problems rather than technical ones: fig core now makes it fully possible.

There were a lot of under-the-hood changes that needed to be made before any of this was possible. Previously, a format could hand the editor up to ten custom hooks to patch up an edit after the fact. Rather than make the hook system extensible, I decided to remove it entirely. Turns out, every hook could be replaced with data the parser already knew, but had thrown away rather than making it accessible to the format. Now, every format is generic rather than special-cased.

![The special cases a format used to need](/docs/devlogs/2-texas-everbearing/texas-everbearing-hooks-retired.svg)

The last thing I want to share is support for new formats, enabled by `fig-quickjs`:

- [sshconfig](https://raw.githubusercontent.com/diaryx-org/fig-quickjs/refs/heads/main/languages/sshconfig.mjs)
- [gitconfig](https://raw.githubusercontent.com/diaryx-org/fig-quickjs/refs/heads/main/languages/gitconfig.mjs)
- [pom.xml](https://raw.githubusercontent.com/diaryx-org/fig-quickjs/refs/heads/main/languages/pom.mjs)
- [OpenStep plist](https://raw.githubusercontent.com/diaryx-org/fig-quickjs/refs/heads/main/languages/openstep.mjs)
- [HCL](https://raw.githubusercontent.com/diaryx-org/fig-quickjs/refs/heads/main/languages/hcl.mjs)

You can configure them by adding them to `~/.config/fig/languages.figl`, like this:

```figl
# ~/.config/fig/languages.figl
language[]
> name = js-hcl
> extensions = [hcl, tf, tfvars]
> command = [fig-quickjs, ~/path/to/hcl.mjs]
+
> name = js-toml
> extensions = [toml]
> command = [fig-quickjs, ~/path/to/toml.mjs]
```

Please enjoy the new Fig update!