// Every language the package ships, registered: the module the tests load
// compiles no format in, and a test reads and writes each through the
// package's language for it, as a caller that imported them all would.
//
// `Format` here is the tests' own table of the formats registration handed
// out, by the names the binding's members had through 5.x — not a package
// export: core has no format members since 6.0.
import { formatByName, registerLanguage, type Format as Handle } from "../src/index.ts";
import canonical from "@diaryx/fig/js/canonical";
import dotenv from "@diaryx/fig-dotenv";
import fig from "@diaryx/fig-figl";
import ini from "@diaryx/fig-ini";
import json from "@diaryx/fig-json";
import nestedtext from "@diaryx/fig-nestedtext";
import plist from "@diaryx/fig-plist";
import properties from "@diaryx/fig-properties";
import toml from "@diaryx/fig-toml";
import yaml from "@diaryx/fig-yaml";
import zon from "@diaryx/fig-zon";

for (const lang of [json, yaml, toml, zon, fig, ini, dotenv, properties, plist, nestedtext, canonical]) registerLanguage(lang);

function byName(name: string): Handle {
  const format = formatByName(name);
  if (format === null) throw new Error(`register-all: no format named ${name}`);
  return format;
}

export const Format = {
  Json: json.format!,
  Jsonc: byName("jsonc"),
  Json5: byName("json5"),
  Yaml: yaml.format!,
  Toml: toml.format!,
  Zon: zon.format!,
  Fig: fig.format!,
  Ini: ini.format!,
  Dotenv: dotenv.format!,
  Properties: properties.format!,
  Plist: plist.format!,
  Nestedtext: nestedtext.format!,
} as const;
export type Format = Handle;
