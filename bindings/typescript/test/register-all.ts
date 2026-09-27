// Every language the package ships, registered: the module the tests load
// compiles no format in, and a test reads and writes each through the
// language that stands in for it, as a caller that imported them all would.
import { registerLanguage } from "../src/index.ts";
import canonical from "@diaryx/fig/languages/canonical";
import dotenv from "@diaryx/fig/languages/dotenv";
import fig from "@diaryx/fig/languages/fig";
import ini from "@diaryx/fig/languages/ini";
import json from "@diaryx/fig/languages/json";
import json5 from "@diaryx/fig/languages/json5";
import nestedtext from "@diaryx/fig/languages/nestedtext";
import plist from "@diaryx/fig/languages/plist";
import properties from "@diaryx/fig/languages/properties";
import toml from "@diaryx/fig/languages/toml";
import yaml from "@diaryx/fig/languages/yaml";
import zon from "@diaryx/fig/languages/zon";

for (const lang of [json, json5, yaml, toml, zon, fig, ini, dotenv, properties, plist, nestedtext, canonical]) registerLanguage(lang);
