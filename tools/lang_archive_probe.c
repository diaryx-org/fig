// A C program that links fig's library and two languages' own libraries
// beside it (libfig.a, libfig_yaml.a, libfig_json.a): it registers each,
// converts a YAML document to JSON and back, and edits it. What
// `cargo xtask abi-check` runs; a link of the language libraries
// against a core that names no language is the point.
#include <stdio.h>
#include <string.h>
#include <fig/yaml.h>
#include <fig/json.h>

#define CHECK(c) do { if (!(c)) { fprintf(stderr, "lang_archive_probe: %s:%d: %s\n", __FILE__, __LINE__, #c); return 1; } } while (0)

int main(void) {
    FigError err = { .size = sizeof err };
    int yaml = -1, json = -1, again = -1;
    CHECK(fig_format_by_name("yaml") < 0);
    CHECK(fig_yaml_register(&yaml, &err) == FIG_STATUS_OK);
    CHECK(fig_json_register(&json, &err) == FIG_STATUS_OK);
    CHECK(fig_yaml_register(&again, &err) == FIG_STATUS_OK && again == yaml);
    CHECK(fig_format_by_name("yaml") == yaml && fig_format_by_name("json5") > 0);

    const char *src = "# a comment\nname: fig\nlist: [1, two]\n";
    FigDocument *doc = NULL;
    CHECK(fig_parse((const uint8_t *)src, strlen(src), yaml, &doc) == FIG_STATUS_OK);
    const uint8_t *out; size_t n;
    CHECK(fig_document_serialize(doc, json, NULL, &out, &n) == FIG_STATUS_OK);
    const char *want = "{\n  \"name\": \"fig\",\n  \"list\": [\n    1,\n    \"two\"\n  ]\n}\n";
    if (n != strlen(want) || memcmp(out, want, n) != 0) { fprintf(stderr, "lang_archive_probe: got %.*s", (int)n, out); return 1; }
    CHECK(fig_document_serialize(doc, yaml, NULL, &out, &n) == FIG_STATUS_OK);
    fig_document_destroy(doc);

    FigEditor *ed = NULL;
    CHECK(fig_editor_create((const uint8_t *)src, strlen(src), yaml, &ed) == FIG_STATUS_OK);
    CHECK(fig_editor_source(ed, &out, &n) == FIG_STATUS_OK && n == strlen(src));
    fig_editor_destroy(ed);

    const char *bad = "a: [1, 2\n";
    FigError perr = { .size = sizeof perr };
    CHECK(fig_parse_ex((const uint8_t *)bad, strlen(bad), yaml, &doc, &perr) == FIG_STATUS_PARSE_ERROR);
    printf("lang_archive_probe: yaml and json register from their own libraries, convert and edit\n");
    return 0;
}
