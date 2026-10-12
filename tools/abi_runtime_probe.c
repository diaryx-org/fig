/* Runtime-language probe (C). Compiled AND RUN by `zig build abi-check`: a
 * language written in C against fig.h's FigLanguageVTable and FigNodeTable is
 * registered, parsed through, converted out of and into, and edited through
 * the C ABI. This is the layout check the symbol diff cannot make — every
 * struct in the runtime section of fig.h is written by this side and read by
 * the Zig side, so a field that drifted between the two would be read here as
 * the wrong bytes. It is also the smallest complete host of a runtime
 * language, which is what the section's prose is describing.
 *
 * The language is `tinykv`: `key=value` lines and `#` comments. */
#include "fig.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(cond)                                                          \
    do {                                                                     \
        if (!(cond)) {                                                       \
            fprintf(stderr, "abi_runtime_probe: %s:%d: %s\n", __FILE__,      \
                    __LINE__, #cond);                                        \
            return 1;                                                        \
        }                                                                    \
    } while (0)

static FigStr str(const char *p, size_t n) {
    FigStr s = { (const uint8_t *)p, n };
    return s;
}

/* A row, as fig_table_rows hands it back (zeroed: a null, no parent). */
static FigRow *add_row(FigTable *t, uint8_t kind, uint32_t parent, size_t start, size_t end) {
    FigRow *r = fig_table_rows(t, 1);
    if (!r) return NULL;
    r->kind = kind;
    r->parent = parent;
    r->start = (uint32_t)start;
    r->end = (uint32_t)end;
    return r;
}

/* Text in the source, by offset: no copy. */
static FigText in_source(size_t off, size_t len) {
    FigText t = { (uint32_t)off, (uint32_t)len };
    return t;
}

static int parse(void *ctx, const char *dialect, FigStr input, FigTable *out, FigError *err) {
    (void)ctx;
    (void)dialect;
    const char *src = (const char *)input.ptr;
    size_t n = input.len;
    if (!add_row(out, FIG_ROW_MAPPING, FIG_ROW_NONE, 0, n)) return FIG_STATUS_OUT_OF_MEMORY;
    uint32_t nrows = 1;

    size_t pending_at[64];
    size_t pending_len[64];
    size_t npending = 0;

    size_t at = 0;
    while (at < n) {
        const char *nl = memchr(src + at, '\n', n - at);
        size_t end = nl ? (size_t)(nl - src) : n;
        size_t len = end - at;
        if (len == 0) { at = end + 1; continue; }
        if (src[at] == '#') {
            size_t s = at + 1;
            while (s < end && src[s] == ' ') s++;
            pending_at[npending] = s;
            pending_len[npending] = end - s;
            npending++;
            at = end + 1;
            continue;
        }
        const char *eq = memchr(src + at, '=', len);
        if (!eq) {
            snprintf((char *)err->message, sizeof err->message, "expected key=value");
            err->message_len = strlen("expected key=value");
            err->byte_offset = at;
            return FIG_STATUS_PARSE_ERROR;
        }
        size_t eqi = (size_t)(eq - src);
        uint32_t kv = nrows++;
        if (!add_row(out, FIG_ROW_KEYVALUE, 0, at, end)) return FIG_STATUS_OUT_OF_MEMORY;
        if (fig_table_sep(out, kv, (uint32_t)eqi, (uint32_t)eqi + 1) != FIG_STATUS_OK) return FIG_STATUS_OUT_OF_MEMORY;
        uint32_t key = nrows++;
        FigRow *k = add_row(out, FIG_ROW_STRING, kv, at, eqi);
        if (!k) return FIG_STATUS_OUT_OF_MEMORY;
        k->text = in_source(at, eqi - at);
        nrows++;
        FigRow *v = add_row(out, FIG_ROW_STRING, kv, eqi + 1, end);
        if (!v) return FIG_STATUS_OUT_OF_MEMORY;
        v->text = in_source(eqi + 1, end - eqi - 1);
        for (size_t i = 0; i < npending; i++) {
            /* A comment's text decoded into the buffer, to cover the copying call. */
            FigText text;
            if (fig_table_string(out, (const uint8_t *)src + pending_at[i], pending_len[i], &text) != FIG_STATUS_OK) return FIG_STATUS_OUT_OF_MEMORY;
            if (fig_table_comment(out, key, FIG_COMMENT_LEADING, FIG_COMMENT_LINE, text) != FIG_STATUS_OK) return FIG_STATUS_OUT_OF_MEMORY;
        }
        npending = 0;
        at = end + 1;
    }
    return 0;
}

/* The bytes a FigText names in a view. */
static const char *text_of(const FigTableView *v, FigText t, size_t *len) {
    if (t.len & FIG_TEXT_BUFFER) {
        *len = t.len & ~FIG_TEXT_BUFFER;
        return (const char *)v->strings + t.off;
    }
    *len = t.len;
    return (const char *)v->source + t.off;
}

static int print(void *ctx, const char *dialect, const FigTableView *table, const FigPrintOptions *options, FigStr *out, FigError *err) {
    (void)ctx;
    (void)dialect;
    (void)options;
    CHECK(table->size >= sizeof(FigTableView));
    size_t cap = 256, len = 0;
    char *buf = malloc(cap);
    for (size_t i = 1; i < table->row_count; i += 3) {
        if (i + 2 >= table->row_count || table->rows[i].kind != FIG_ROW_KEYVALUE ||
            table->rows[i + 1].kind != FIG_ROW_STRING || table->rows[i + 2].kind != FIG_ROW_STRING) {
            const char *m = "tinykv holds a flat string map";
            snprintf((char *)err->message, sizeof err->message, "%s", m);
            err->message_len = strlen(m);
            free(buf);
            return FIG_STATUS_UNSUPPORTED_FORMAT;
        }
        for (size_t c = 0; c < table->comment_count; c++) {
            const FigTableComment *cr = &table->comments[c];
            if (cr->node == i + 1 && cr->slot == FIG_COMMENT_LEADING) {
                size_t tl;
                const char *tp = text_of(table, cr->text, &tl);
                size_t need = len + tl + 3;
                if (need > cap) { cap = need * 2; buf = realloc(buf, cap); }
                len += (size_t)snprintf(buf + len, cap - len, "# %.*s\n", (int)tl, tp);
            }
        }
        size_t kl, vl;
        const char *kp = text_of(table, table->rows[i + 1].text, &kl);
        const char *vp = text_of(table, table->rows[i + 2].text, &vl);
        size_t need = len + kl + vl + 3;
        if (need > cap) { cap = need * 2; buf = realloc(buf, cap); }
        len += (size_t)snprintf(buf + len, cap - len, "%.*s=%.*s\n", (int)kl, kp, (int)vl, vp);
    }
    out->ptr = (const uint8_t *)buf;
    out->len = len;
    return 0;
}

static void free_bytes(void *ctx, FigStr bytes) {
    (void)ctx;
    free((void *)bytes.ptr);
}

/* `cbars`: one line of words joined by ` | `, a trailing `|` allowed — an
 * open sequence whose row names its separator, so the item edits work on it
 * through the C ABI, and `fig_editor_insert_seq_item` with them. */
static int parse_bars(void *ctx, const char *dialect, FigStr input, FigTable *out, FigError *err) {
    (void)ctx;
    (void)dialect;
    (void)err;
    const char *src = (const char *)input.ptr;
    size_t n = input.len;
    while (n > 0 && (src[n - 1] == '\n' || src[n - 1] == ' ')) n--;
    FigRow *root = add_row(out, FIG_ROW_SEQUENCE, FIG_ROW_NONE, 0, n);
    if (!root) return FIG_STATUS_OUT_OF_MEMORY;
    root->flags = FIG_ROW_OPEN;
    FigText sep;
    if (fig_table_string(out, (const uint8_t *)" | ", 3, &sep) != FIG_STATUS_OK) return FIG_STATUS_OUT_OF_MEMORY;
    if (fig_table_item_sep(out, 0, sep) != FIG_STATUS_OK) return FIG_STATUS_OUT_OF_MEMORY;
    size_t at = 0;
    while (at < n) {
        while (at < n && (src[at] == ' ' || src[at] == '|')) at++;
        size_t end = at;
        while (end < n && src[end] != ' ' && src[end] != '|') end++;
        if (end == at) break;
        FigRow *r = add_row(out, FIG_ROW_STRING, 0, at, end);
        if (!r) return FIG_STATUS_OUT_OF_MEMORY;
        r->text = in_source(at, end - at);
        at = end;
    }
    return 0;
}

static int bars_probe(void) {
    static const FigSyntax syntax = {
        /* size */ sizeof(FigSyntax),
        /* comments */ { { "#", NULL, NULL }, { NULL, NULL, NULL }, { NULL, NULL, NULL } },
        /* kv_sep */ ": ",
        /* flow_kv_sep_from_siblings */ false,
        /* flow_map_pad */ NULL,
        /* key_sigil */ 0,
        /* empty_map_literal */ NULL,
        /* block_seq_editable */ true,
        /* flow_containers */ false,
        /* indent_unit */ NULL,
        /* seq_item_marker */ NULL,
        /* closed_containers */ { NULL, NULL, NULL, NULL },
        /* single_line_block_mapping */ false,
        /* bare_document_mapping */ false,
        /* flow_map_open */ NULL,
        /* flow_map_close */ NULL,
        /* structural_indent */ false,
        /* section_noun */ NULL,
        /* section_header */ { NULL, NULL, NULL, NULL, NULL, true },
        /* merge_key */ NULL,
    };
    static const char *const extensions[] = { "cbars", NULL };
    static const FigDialectDesc dialects[] = {
        { "cbars", extensions, 2, "", NULL },
    };
    static const char sample[] = "a | b |\n";
    const FigStr samples[] = { str(sample, sizeof sample - 1) };
    FigLanguageVTable vt;
    memset(&vt, 0, sizeof vt);
    vt.version = FIG_LANGUAGE_VTABLE_VERSION;
    vt.size = sizeof vt;
    vt.name = "cbars";
    vt.caps = FIG_CAP_READ | FIG_CAP_EDIT;
    vt.max_mapping_depth = 0;
    vt.syntax = &syntax;
    vt.dialects = dialects;
    vt.dialect_count = 1;
    vt.dialect_size = sizeof(FigDialectDesc);
    vt.samples = samples;
    vt.sample_count = 1;
    vt.parse = parse_bars;
    vt.free_bytes = free_bytes;
    int format = 0;
    FigError err;
    memset(&err, 0, sizeof err);
    err.size = sizeof err;
    if (fig_language_register(&vt, &format, &err) != FIG_STATUS_OK) {
        fprintf(stderr, "abi_runtime_probe: register cbars: %.*s\n", (int)err.message_len, (const char *)err.message);
        return 1;
    }
    static const char src[] = "a | b | c |\n";
    FigEditor *ed = NULL;
    CHECK(fig_editor_create((const uint8_t *)src, sizeof src - 1, format, &ed) == FIG_STATUS_OK);
    CHECK(fig_editor_insert_seq_item(ed, NULL, 0, 1, (const uint8_t *)"x", 1) == FIG_STATUS_OK);
    CHECK(fig_editor_remove_seq_item(ed, NULL, 0, 0) == FIG_STATUS_OK);
    CHECK(fig_editor_append_seq(ed, NULL, 0, (const uint8_t *)"d", 1) == FIG_STATUS_OK);
    CHECK(fig_editor_insert_seq_item(ed, NULL, 0, 9, (const uint8_t *)"y", 1) == FIG_STATUS_NOT_FOUND);
    static const char edited[] = "x | b | c | d |\n";
    const uint8_t *out;
    size_t out_len;
    CHECK(fig_editor_source(ed, &out, &out_len) == FIG_STATUS_OK);
    CHECK(out_len == sizeof edited - 1 && memcmp(out, edited, out_len) == 0);
    fig_editor_destroy(ed);
    return 0;
}

int main(void) {
    static const FigSyntax syntax = {
        /* size */ sizeof(FigSyntax),
        /* comments */ { { "#", NULL, NULL }, { NULL, NULL, NULL }, { NULL, NULL, NULL } },
        /* kv_sep */ "=",
        /* flow_kv_sep_from_siblings */ false,
        /* flow_map_pad */ NULL,
        /* key_sigil */ 0,
        /* empty_map_literal */ "{}",
        /* block_seq_editable */ true,
        /* flow_containers */ false,
        /* indent_unit */ NULL,
        /* seq_item_marker */ NULL,
        /* closed_containers */ { NULL, NULL, NULL, NULL },
        /* single_line_block_mapping */ false,
        /* bare_document_mapping */ true,
        /* flow_map_open */ NULL,
        /* flow_map_close */ NULL,
        /* structural_indent */ false,
        /* section_noun */ NULL,
        /* section_header */ { NULL, NULL, NULL, NULL, NULL, true },
        /* merge_key */ NULL,
    };
    static const char *const extensions[] = { "tkv", NULL };
    static const FigDialectDesc dialects[] = {
        { "tinykv", extensions, 2, "", NULL },
    };
    static const char sample1[] = "a=1\nb=two\n";
    static const char sample2[] = "# top\nk=v\n";
    const FigStr samples[] = { str(sample1, sizeof sample1 - 1), str(sample2, sizeof sample2 - 1) };

    FigLanguageVTable vt;
    memset(&vt, 0, sizeof vt);
    vt.version = FIG_LANGUAGE_VTABLE_VERSION;
    vt.size = sizeof vt;
    vt.name = "tinykv";
    vt.caps = FIG_CAP_READ | FIG_CAP_EDIT | FIG_CAP_SERIALIZE;
    vt.max_mapping_depth = 0; /* flat: no mapping inside the root (FIG_DEPTH_NONE would be unbounded) */
    vt.syntax = &syntax;
    vt.dialects = dialects;
    vt.dialect_count = 1;
    vt.dialect_size = sizeof(FigDialectDesc);
    vt.samples = samples;
    vt.sample_count = 2;
    vt.parse = parse;
    vt.print = print;
    vt.free_bytes = free_bytes;

    CHECK(fig_language_vtable_version() == FIG_LANGUAGE_VTABLE_VERSION);

    int format = 0;
    FigError err;
    memset(&err, 0, sizeof err);
    err.size = sizeof err;
    FigStatus st = fig_language_register(&vt, &format, &err);
    if (st != FIG_STATUS_OK) {
        fprintf(stderr, "abi_runtime_probe: register: %d: %.*s\n", (int)st, (int)err.message_len, (const char *)err.message);
        return 1;
    }
    CHECK(format >= FIG_FORMAT_RUNTIME_BASE);
    CHECK(fig_format_by_name("tinykv") == format);
    CHECK(fig_format_capabilities(format) == (FIG_CAP_READ | FIG_CAP_EDIT | FIG_CAP_SERIALIZE));

    static const char src[] = "# note\nx=1\ny=two\n";
    FigDocument *doc = NULL;
    CHECK(fig_parse((const uint8_t *)src, sizeof src - 1, format, &doc) == FIG_STATUS_OK);
    FigNodeId root = fig_document_root(doc);
    CHECK(fig_node_kind(doc, root) == FIG_NODE_MAPPING);
    CHECK(fig_node_child_count(doc, root) == 2);

    const uint8_t *out;
    size_t out_len;
    CHECK(fig_document_serialize(doc, format, NULL, &out, &out_len) == FIG_STATUS_OK);
    CHECK(out_len == sizeof src - 1 && memcmp(out, src, out_len) == 0);
    if (fig_format_capabilities(FIG_FORMAT_JSON) & FIG_CAP_SERIALIZE) {
        static const char want[] = "{\n  \"x\": \"1\",\n  \"y\": \"two\"\n}\n";
        CHECK(fig_document_serialize(doc, FIG_FORMAT_JSON, NULL, &out, &out_len) == FIG_STATUS_OK);
        CHECK(out_len == sizeof want - 1 && memcmp(out, want, out_len) == 0);
    }
    fig_document_destroy(doc);

    FigEditor *ed = NULL;
    CHECK(fig_editor_create((const uint8_t *)src, sizeof src - 1, format, &ed) == FIG_STATUS_OK);
    FigPathSegment seg = { FIG_SEGMENT_KEY, (const uint8_t *)"x", 1, 0 };
    CHECK(fig_editor_replace_val(ed, &seg, 1, (const uint8_t *)"10", 2) == FIG_STATUS_OK);
    CHECK(fig_editor_insert_key(ed, NULL, 0, (const uint8_t *)"z", 1, (const uint8_t *)"3", 1) == FIG_STATUS_OK);
    static const char edited[] = "# note\nx=10\ny=two\nz=3\n";
    CHECK(fig_editor_source(ed, &out, &out_len) == FIG_STATUS_OK);
    CHECK(out_len == sizeof edited - 1 && memcmp(out, edited, out_len) == 0);
    fig_editor_destroy(ed);

    /* A second registration of the name is refused, with the reason. */
    int again = 0;
    CHECK(fig_language_register(&vt, &again, &err) == FIG_STATUS_INVALID_ARGUMENT);
    CHECK(strstr((const char *)err.message, "already registered") != NULL);

    if (bars_probe() != 0) return 1;

    puts("abi_runtime_probe: a C-hosted runtime language registers, parses, converts and edits through fig.h");
    return 0;
}
