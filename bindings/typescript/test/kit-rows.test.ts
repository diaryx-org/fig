// The JavaScript kit's row writer, held to the fixtures every binding's
// kit is held to (`kit/testdata/rows/`, whose README names the ops): rows
// written in any order, linked, put by a duplicate policy, and finished
// into the pre-order table the file gives.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import * as fig from "@diaryx/fig/kit";

const dir = join(import.meta.dirname, "..", "..", "..", "kit", "testdata", "rows");

type Op = [string, ...any[]];

/** What a finished table is on the wire: the lists a fixture leaves out
 *  when they are empty, left out here too. */
function wire(t: fig.Table): Record<string, unknown> {
  const out: Record<string, unknown> = { rows: t.rows, regions: t.regions, mentions: t.mentions, comments: t.comments };
  if (t.directives.length > 0) out.directives = t.directives;
  return JSON.parse(JSON.stringify(out));
}

for (const name of readdirSync(dir).filter((f) => f.endsWith(".json")).sort()) {
  test(`rows: ${name}`, () => {
    const fx = JSON.parse(readFileSync(join(dir, name), "utf8"));
    const t = fig.table();
    const puts: unknown[] = [];
    for (const [op, ...a] of fx.ops as Op[]) {
      switch (op) {
        case "add":
          t.row(a[0], a[1], a[2], a[3]);
          break;
        case "append":
          t.append(a[0], a[1]);
          break;
        case "first":
          t.first(a[0], a[1]);
          break;
        case "next":
          t.next(a[0], a[1]);
          break;
        case "put":
          try {
            const r = t.put(a[0], a[1], a[2], a[3]);
            puts.push([r.entry, r.repeat]);
          } catch (e) {
            if (!fig.isFailure(e)) throw e;
            puts.push("error");
          }
          break;
        case "child_by_key":
          assert.equal(t.childByKey(a[0], a[1]), a[2], `child_by_key ${a[1]}`);
          break;
        case "comment":
          t.comment(a[0], a[1], a[2], a[3]);
          break;
        case "region":
          t.region(a[0], a[1], a[2]);
          break;
        case "mention":
          t.mention(a[0], a[1], a[2]);
          break;
        case "finish":
          t.finish(a[0]);
          break;
        default:
          throw new Error(`unknown op ${op}`);
      }
    }
    if (fx.puts) assert.deepEqual(puts, fx.puts);
    assert.deepEqual(wire(t), fx.table);
  });
}

test("rows: a table written in pre-order with parents is the same table before and after finish", () => {
  const t = fig.table();
  const m = t.row("mapping", null, [0, 3]);
  const kv = t.row("keyvalue", m, [0, 3]);
  t.row("string", kv, [0, 1], { text: "a" });
  t.row("string", kv, [2, 3], { text: "1" });
  t.comment(kv, "leading", "line", "c");
  const before = JSON.stringify(t);
  t.finish(m);
  assert.equal(JSON.stringify(t), before);
});
