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

static const FigStr STR_NONE = { NULL, FIG_LEN_NONE };
static const FigSpan SPAN_NONE = { FIG_OFFSET_NONE, FIG_OFFSET_NONE };

static FigNodeRow row(int kind, uint32_t parent, size_t start, size_t end) {
    FigNodeRow r;
    r.kind = kind;
    r.ext_kind = FIG_EXT_NONE;
    r.parent = parent;
    r.span.start = start;
    r.span.end = end;
    r.text = STR_NONE;
    r.anchor = STR_NONE;
    r.anchor_span = SPAN_NONE;
    r.tag = STR_NONE;
    r.tag_span = SPAN_NONE;
    r.marker = SPAN_NONE;
    r.sep = SPAN_NONE;
    return r;
}

static int parse(void *ctx, const char *dialect, FigStr input, FigNodeTable *out, FigError *err) {
    (void)ctx;
    (void)dialect;
    const char *src = (const char *)input.ptr;
    size_t n = input.len;
    /* Three rows per line plus the root is an upper bound. */
    FigNodeRow *rows = calloc(3 * (n + 1) + 1, sizeof *rows);
    FigCommentRow *comments = calloc(n + 1, sizeof *comments);
    size_t nrows = 0, ncomments = 0;
    rows[nrows++] = row(FIG_NODE_MAPPING, FIG_ROW_NONE, 0, n);

    const char *pending[64];
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
            pending[npending] = src + s;
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
            free(rows);
            free(comments);
            return FIG_STATUS_PARSE_ERROR;
        }
        size_t eqi = (size_t)(eq - src);
        uint32_t kv = (uint32_t)nrows;
        rows[nrows] = row(FIG_NODE_KEYVALUE, 0, at, end);
        rows[nrows].sep.start = eqi;
        rows[nrows].sep.end = eqi + 1;
        nrows++;
        uint32_t key = (uint32_t)nrows;
        rows[nrows] = row(FIG_NODE_STRING, kv, at, eqi);
        rows[nrows].text = str(src + at, eqi - at);
        nrows++;
        rows[nrows] = row(FIG_NODE_STRING, kv, eqi + 1, end);
        rows[nrows].text = str(src + eqi + 1, end - eqi - 1);
        nrows++;
        for (size_t i = 0; i < npending; i++) {
            comments[ncomments].node = key;
            comments[ncomments].slot = FIG_COMMENT_LEADING;
            comments[ncomments].style = FIG_COMMENT_LINE;
            comments[ncomments].text = str(pending[i], pending_len[i]);
            ncomments++;
        }
        npending = 0;
        at = end + 1;
    }
    out->rows = rows;
    out->row_count = nrows;
    out->regions = NULL;
    out->region_count = 0;
    out->mentions = NULL;
    out->mention_count = 0;
    out->comments = ncomments ? comments : NULL;
    out->comment_count = ncomments;
    out->directives = NULL;
    out->directive_count = 0;
    out->owner = NULL;
    if (!ncomments) free(comments);
    return 0;
}

static void free_table(void *ctx, FigNodeTable *table) {
    (void)ctx;
    free((void *)table->rows);
    free((void *)table->comments);
}

static int print(void *ctx, const char *dialect, const FigNodeTable *table, const FigPrintOptions *options, FigStr *out, FigError *err) {
    (void)ctx;
    (void)dialect;
    (void)options;
    size_t cap = 256, len = 0;
    char *buf = malloc(cap);
    for (size_t i = 1; i < table->row_count; i += 3) {
        if (i + 2 >= table->row_count || table->rows[i].kind != FIG_NODE_KEYVALUE ||
            table->rows[i + 1].kind != FIG_NODE_STRING || table->rows[i + 2].kind != FIG_NODE_STRING) {
            const char *m = "tinykv holds a flat string map";
            snprintf((char *)err->message, sizeof err->message, "%s", m);
            err->message_len = strlen(m);
            free(buf);
            return FIG_STATUS_UNSUPPORTED_FORMAT;
        }
        const FigNodeRow *key = &table->rows[i + 1];
        const FigNodeRow *val = &table->rows[i + 2];
        for (size_t c = 0; c < table->comment_count; c++) {
            const FigCommentRow *cr = &table->comments[c];
            if (cr->node == i + 1 && cr->slot == FIG_COMMENT_LEADING) {
                size_t need = len + cr->text.len + 3;
                if (need > cap) { cap = need * 2; buf = realloc(buf, cap); }
                len += (size_t)snprintf(buf + len, cap - len, "# %.*s\n", (int)cr->text.len, (const char *)cr->text.ptr);
            }
        }
        size_t need = len + key->text.len + val->text.len + 3;
        if (need > cap) { cap = need * 2; buf = realloc(buf, cap); }
        len += (size_t)snprintf(buf + len, cap - len, "%.*s=%.*s\n", (int)key->text.len, (const char *)key->text.ptr,
                                (int)val->text.len, (const char *)val->text.ptr);
    }
    out->ptr = (const uint8_t *)buf;
    out->len = len;
    return 0;
}

static void free_bytes(void *ctx, FigStr bytes) {
    (void)ctx;
    free((void *)bytes.ptr);
}

int main(void) {
    static const FigSyntax syntax = {
        /* comments */ { 0, { "#", NULL, NULL }, { NULL, NULL, NULL } },
        /* kv_sep */ "=",
        /* flow_kv_sep_from_siblings */ false,
        /* flow_map_pad */ NULL,
        /* key_style */ 0,
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
        /* section_noun */ -1,
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
    vt.name = "tinykv";
    vt.caps = FIG_CAP_READ | FIG_CAP_EDIT | FIG_CAP_SERIALIZE;
    vt.max_mapping_depth = 0; /* flat: no mapping inside the root (FIG_DEPTH_NONE would be unbounded) */
    vt.syntax = &syntax;
    vt.dialects = dialects;
    vt.dialect_count = 1;
    vt.samples = samples;
    vt.sample_count = 2;
    vt.parse = parse;
    vt.print = print;
    vt.free_table = free_table;
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
    FigPathSegment seg = { 0, (const uint8_t *)"x", 1, 0 };
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

    puts("abi_runtime_probe: a C-hosted runtime language registers, parses, converts and edits through fig.h");
    return 0;
}
