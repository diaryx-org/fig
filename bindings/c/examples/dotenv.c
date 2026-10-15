/* dotenv, written in C against fig.h: the twin of fig's compiled `dotenv`,
 * row for row, as the kit's dotenv twins are in Zig, Rust and JavaScript.
 * What it uses of the kit is what C gets: rows written as the parser meets
 * them and linked, a repeated key found with fig_table_child_by_key, and
 * fig_table_finish to put them in pre-order. fig's core tests register it
 * (kit_dotenv_c_register) and hold it to the compiled dotenv table for
 * table.
 *
 *   * a key is a bash identifier, [A-Za-z_][A-Za-z0-9_]*; an `export `
 *     before one is recognized and discarded;
 *   * a value is unquoted (trimmed, to the end of the line or to a `#` that
 *     a space or tab precedes), double-quoted (\n \t \r \\ \" decoded, may
 *     span lines) or single-quoted (raw, may span lines);
 *   * a full-line `#` is a comment, leading for the next key or dangling on
 *     the root; a `#` after a value on its line is its trailing one;
 *   * a repeated key keeps the first entry's place and takes the last value.
 */
#include "fig.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BARE_CR "a bare `\\r` must be followed by `\\n`; line endings must be `\\n` or `\\r\\n`"
#define UNCLOSED "unclosed quoted value; expected a matching `\"`/`'` before the end of the file"

typedef struct Parse {
    const uint8_t *src;
    size_t n;
    size_t pos;
    FigTable *t;
    FigError *err;
    /* The comments waiting for a key: where each one's text is. */
    size_t *pending_at, *pending_len;
    size_t npending, cap;
    /* The node a comment on its line trails, and the root's last entry. */
    uint32_t last, last_entry;
    /* How many rows have been written. */
    uint32_t rows;
} Parse;

static int refuse(Parse *p, const char *message, size_t at) {
    size_t n = strlen(message);
    if (n >= sizeof p->err->message) n = sizeof p->err->message - 1;
    memcpy(p->err->message, message, n);
    p->err->message[n] = 0;
    p->err->message_len = n;
    p->err->byte_offset = at;
    return FIG_STATUS_PARSE_ERROR;
}

static int is_key_start(uint8_t c) {
    return c == '_' || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z');
}

static int is_key(uint8_t c) {
    return is_key_start(c) || (c >= '0' && c <= '9');
}

static int is_hs(uint8_t c) {
    return c == ' ' || c == '\t';
}

static void hs(Parse *p) {
    while (p->pos < p->n && is_hs(p->src[p->pos])) p->pos++;
}

static int at(Parse *p, const char *s) {
    size_t k = strlen(s);
    return p->pos + k <= p->n && memcmp(p->src + p->pos, s, k) == 0;
}

/* A new row of `kind` over [start, end), linked to nothing; its id. */
static int new_row(Parse *p, uint8_t kind, size_t start, size_t end, uint32_t *id) {
    static const FigRow blank = { 0, 0, 0, FIG_ROW_NONE, 0, 0, { 0, FIG_ROW_NONE }, FIG_ROW_NONE, FIG_ROW_NONE };
    FigRow *r = fig_table_rows(p->t, 1);
    if (!r) return FIG_STATUS_OUT_OF_MEMORY;
    *r = blank;
    r->kind = kind;
    r->start = (uint32_t)start;
    r->end = (uint32_t)end;
    *id = p->rows++;
    return FIG_STATUS_OK;
}

static int comment(Parse *p, uint32_t node, uint8_t slot, size_t off, size_t len) {
    FigText text = { (uint32_t)off, (uint32_t)len };
    return fig_table_comment(p->t, node, slot, FIG_COMMENT_LINE, text);
}

/* Give every waiting comment to `node`, in `slot`. */
static int flush(Parse *p, uint32_t node, uint8_t slot) {
    for (size_t i = 0; i < p->npending; i++) {
        int st = comment(p, node, slot, p->pending_at[i], p->pending_len[i]);
        if (st != FIG_STATUS_OK) return st;
    }
    p->npending = 0;
    return FIG_STATUS_OK;
}

/* Spaces, line endings and `#` comments, each comment bound: trailing on the
 * last value when it is on that value's line, else waiting. */
static int trivia(Parse *p) {
    while (p->pos < p->n) {
        uint8_t c = p->src[p->pos];
        if (is_hs(c) || c == '\n') {
            p->pos++;
        } else if (c == '\r') {
            if (p->pos + 1 < p->n && p->src[p->pos + 1] == '\n') p->pos += 2;
            else return refuse(p, BARE_CR, p->pos);
        } else if (c == '#') {
            size_t s = p->pos, body = s + 1, e = body;
            while (e < p->n && p->src[e] != '\n' && p->src[e] != '\r') e++;
            p->pos = e;
            size_t ts = body, te = e;
            while (ts < te && (is_hs(p->src[ts]) || p->src[ts] == '\r')) ts++;
            while (te > ts && (is_hs(p->src[te - 1]) || p->src[te - 1] == '\r')) te--;
            FigRow *last = p->last == FIG_ROW_NONE ? NULL : fig_table_row(p->t, p->last);
            if (last && !memchr(p->src + last->end, '\n', s - last->end)) {
                int st = comment(p, p->last, FIG_COMMENT_TRAILING, ts, te - ts);
                if (st != FIG_STATUS_OK) return st;
            } else {
                if (p->npending == p->cap) {
                    p->cap = p->cap ? p->cap * 2 : 8;
                    p->pending_at = realloc(p->pending_at, p->cap * sizeof *p->pending_at);
                    p->pending_len = realloc(p->pending_len, p->cap * sizeof *p->pending_len);
                    if (!p->pending_at || !p->pending_len) return FIG_STATUS_OUT_OF_MEMORY;
                }
                p->pending_at[p->npending] = ts;
                p->pending_len[p->npending] = te - ts;
                p->npending++;
                p->last = FIG_ROW_NONE;
            }
        } else {
            break;
        }
    }
    return FIG_STATUS_OK;
}

/* Only spaces, a `#` comment or the line's end may follow a closing quote. */
static int after_quoted(Parse *p) {
    size_t save = p->pos;
    hs(p);
    if (!(p->pos >= p->n || at(p, "#") || at(p, "\n") || at(p, "\r")))
        return refuse(p, "unexpected content after this quoted value; only a `#` comment may follow it on the same line", p->pos);
    p->pos = save;
    return FIG_STATUS_OK;
}

/* A quoted value opened by `q`: its row, its text decoded into the table. */
static int quoted(Parse *p, uint8_t q, uint32_t *id) {
    size_t s = p->pos, i = s + 1;
    for (;;) {
        if (i >= p->n) return refuse(p, UNCLOSED, i);
        uint8_t c = p->src[i];
        if (c == '\r') {
            if (i + 1 < p->n && p->src[i + 1] == '\n') i += 2;
            else return refuse(p, BARE_CR, i);
        } else if (q == '"' && c == '\\') {
            if (i + 1 >= p->n) return refuse(p, UNCLOSED, i);
            i += 2;
        } else if (c == q) {
            i += 1;
            break;
        } else {
            i += 1;
        }
    }
    p->pos = i;
    int st = after_quoted(p);
    if (st != FIG_STATUS_OK) return st;
    const uint8_t *inner = p->src + s + 1;
    size_t len = i - 1 - (s + 1), n = 0;
    uint8_t *out = malloc(len ? len : 1);
    if (!out) return FIG_STATUS_OUT_OF_MEMORY;
    for (size_t j = 0; j < len;) {
        uint8_t ch = inner[j];
        if (ch == '\r' && j + 1 < len && inner[j + 1] == '\n') {
            out[n++] = '\n';
            j += 2;
        } else if (q == '"' && ch == '\\') {
            uint8_t e = inner[j + 1];
            switch (e) {
            case 'n': out[n++] = '\n'; break;
            case 't': out[n++] = '\t'; break;
            case 'r': out[n++] = '\r'; break;
            case '\\': out[n++] = '\\'; break;
            case '"': out[n++] = '"'; break;
            default:
                free(out);
                return refuse(p, "invalid escape in a double-quoted value; supported: \\n \\t \\r \\\\ \\\" — use a single-quoted value for raw text with backslashes", i);
            }
            j += 2;
        } else {
            out[n++] = ch;
            j += 1;
        }
    }
    FigText text;
    st = fig_table_string(p->t, out, n, &text);
    free(out);
    if (st != FIG_STATUS_OK) return st;
    st = new_row(p, FIG_ROW_STRING, s, i, id);
    if (st != FIG_STATUS_OK) return st;
    fig_table_row(p->t, *id)->text = text;
    return FIG_STATUS_OK;
}

/* An unquoted value: to the line's end, or to a `#` a space or tab
 * precedes, trimmed. */
static int bare(Parse *p, uint32_t *id) {
    size_t s = p->pos;
    while (p->pos < p->n) {
        uint8_t c = p->src[p->pos];
        if (c == '\n' || c == '\r') break;
        if (c == '#' && p->pos > 0 && is_hs(p->src[p->pos - 1])) break;
        p->pos++;
    }
    size_t ts = s, te = p->pos;
    while (ts < te && is_hs(p->src[ts])) ts++;
    while (te > ts && is_hs(p->src[te - 1])) te--;
    int st = new_row(p, FIG_ROW_STRING, ts, te, id);
    if (st != FIG_STATUS_OK) return st;
    FigText text = { (uint32_t)ts, (uint32_t)(te - ts) };
    fig_table_row(p->t, *id)->text = text;
    return FIG_STATUS_OK;
}

/* One `KEY=value` line, put in the root by its key. */
static int entry(Parse *p) {
    size_t s = p->pos;
    if (at(p, "export")) {
        size_t q = p->pos + 6, r = q;
        while (r < p->n && is_hs(p->src[r])) r++;
        if (r > q && r < p->n && is_key_start(p->src[r])) p->pos = r;
    }
    if (p->pos >= p->n || !is_key_start(p->src[p->pos])) {
        p->pos = s;
        if (at(p, "=")) return refuse(p, "unexpected content here; expected `KEY=value` (optionally `export KEY=value`)", p->pos);
        return refuse(p, "not a valid key here; a dotenv key is a bash identifier (`[A-Za-z_][A-Za-z0-9_]*`)", p->pos);
    }
    size_t ks = p->pos;
    while (p->pos < p->n && is_key(p->src[p->pos])) p->pos++;
    size_t ke = p->pos;
    uint32_t key, value;
    int st = new_row(p, FIG_ROW_STRING, ks, ke, &key);
    if (st != FIG_STATUS_OK) return st;
    FigText ktext = { (uint32_t)ks, (uint32_t)(ke - ks) };
    fig_table_row(p->t, key)->text = ktext;
    hs(p);
    if (!at(p, "=")) return refuse(p, "expected `=` after this key; every dotenv line is `KEY=value`", p->pos);
    p->pos++;
    hs(p);
    if (at(p, "\"")) st = quoted(p, '"', &value);
    else if (at(p, "'")) st = quoted(p, '\'', &value);
    else st = bare(p, &value);
    if (st != FIG_STATUS_OK) return st;
    /* The waiting comments lead this key, kept or not. */
    st = flush(p, key, FIG_COMMENT_LEADING);
    if (st != FIG_STATUS_OK) return st;
    uint32_t repeat = fig_table_child_by_key(p->t, 0, p->src + ks, ke - ks);
    if (repeat != FIG_ROW_NONE) {
        /* The first entry keeps its place and takes this value. */
        uint32_t first_key = fig_table_row(p->t, repeat)->first_child;
        fig_table_row(p->t, first_key)->next_sibling = value;
    } else {
        uint32_t kv;
        uint32_t vend = fig_table_row(p->t, value)->end;
        st = new_row(p, FIG_ROW_KEYVALUE, ks, vend, &kv);
        if (st != FIG_STATUS_OK) return st;
        FigRow *k = fig_table_row(p->t, kv);
        k->first_child = key;
        fig_table_row(p->t, key)->next_sibling = value;
        if (p->last_entry == FIG_ROW_NONE) fig_table_row(p->t, 0)->first_child = kv;
        else fig_table_row(p->t, p->last_entry)->next_sibling = kv;
        p->last_entry = kv;
    }
    fig_table_row(p->t, value)->next_sibling = FIG_ROW_NONE;
    p->last = value;
    return FIG_STATUS_OK;
}

static int parse(void *ctx, const char *dialect, FigStr input, FigTable *out, FigError *err) {
    (void)ctx;
    (void)dialect;
    Parse p;
    memset(&p, 0, sizeof p);
    p.src = input.ptr;
    p.n = input.len;
    p.t = out;
    p.err = err;
    p.last = FIG_ROW_NONE;
    p.last_entry = FIG_ROW_NONE;
    uint32_t root;
    int st = new_row(&p, FIG_ROW_MAPPING, 0, p.n, &root);
    if (p.n >= 3 && memcmp(p.src, "\xef\xbb\xbf", 3) == 0) p.pos = 3;
    while (st == FIG_STATUS_OK) {
        st = trivia(&p);
        if (st != FIG_STATUS_OK || p.pos >= p.n) break;
        st = entry(&p);
    }
    if (st == FIG_STATUS_OK) st = flush(&p, root, FIG_COMMENT_DANGLING);
    if (st == FIG_STATUS_OK) st = fig_table_finish(out, root);
    free(p.pending_at);
    free(p.pending_len);
    return st;
}

/* ── the printer ─────────────────────────────────────────────────────────── */

typedef struct Buf {
    char *p;
    size_t len, cap;
} Buf;

static int put(Buf *b, const char *s, size_t n) {
    if (b->len + n > b->cap) {
        b->cap = (b->len + n) * 2 + 64;
        b->p = realloc(b->p, b->cap);
        if (!b->p) return 0;
    }
    memcpy(b->p + b->len, s, n);
    b->len += n;
    return 1;
}

static const char *text_of(const FigTableView *v, FigText t, size_t *len) {
    if (t.len == FIG_ROW_NONE) {
        *len = 0;
        return "";
    }
    if (t.len & FIG_TEXT_BUFFER) {
        *len = t.len & ~FIG_TEXT_BUFFER;
        return (const char *)v->strings + t.off;
    }
    *len = t.len;
    return (const char *)v->source + t.off;
}

static int needs_quoting(const char *v, size_t n) {
    if (n == 0) return 0;
    if (is_hs((uint8_t)v[0]) || is_hs((uint8_t)v[n - 1])) return 1;
    for (size_t i = 0; i < n; i++)
        if (strchr("\n\r\"\\#", v[i]) && v[i]) return 1;
    return 0;
}

static int write_value(Buf *b, const FigTableView *v, uint32_t id) {
    const FigRow *r = &v->rows[id];
    size_t n;
    const char *s = text_of(v, r->text, &n);
    if (r->kind == FIG_ROW_TRUE) return put(b, "true", 4);
    if (r->kind == FIG_ROW_FALSE) return put(b, "false", 5);
    if (r->kind == FIG_ROW_STRING && needs_quoting(s, n)) {
        if (!put(b, "\"", 1)) return 0;
        for (size_t i = 0; i < n; i++) {
            const char *e = s[i] == '\n' ? "\\n" : s[i] == '\r' ? "\\r" : s[i] == '\t' ? "\\t" : s[i] == '"' ? "\\\"" : s[i] == '\\' ? "\\\\" : NULL;
            if (!(e ? put(b, e, 2) : put(b, s + i, 1))) return 0;
        }
        return put(b, "\"", 1);
    }
    if (r->kind == FIG_ROW_STRING || r->kind == FIG_ROW_INT || r->kind == FIG_ROW_FLOAT || r->kind >= FIG_ROW_EXT) return put(b, s, n);
    return -1;
}

/* Each comment on `node` in `slot` as a `# text` line. */
static int comment_lines(Buf *b, const FigTableView *v, uint32_t node, uint8_t slot) {
    for (size_t i = 0; i < v->comment_count; i++) {
        const FigTableComment *c = &v->comments[i];
        if (c->node != node || c->slot != slot) continue;
        size_t n;
        const char *s = text_of(v, c->text, &n);
        if (!put(b, "#", 1)) return 0;
        if (n > 0 && !(put(b, " ", 1) && put(b, s, n))) return 0;
        if (!put(b, "\n", 1)) return 0;
    }
    return 1;
}

static int fail_print(Buf *b, FigError *err, const char *m) {
    free(b->p);
    size_t n = strlen(m);
    memcpy(err->message, m, n + 1);
    err->message_len = n;
    return FIG_STATUS_UNSUPPORTED_FORMAT;
}

static int print(void *ctx, const char *dialect, const FigTableView *v, const FigPrintOptions *options, FigStr *out, FigError *err) {
    (void)ctx;
    (void)dialect;
    (void)options;
    Buf b = { NULL, 0, 0 };
    if (v->rows[0].kind != FIG_ROW_MAPPING) {
        int ok = write_value(&b, v, 0);
        if (ok <= 0) return fail_print(&b, err, "dotenv writes a string value alone");
    } else {
        for (uint32_t kv = 1; kv < v->row_count; kv++) {
            if (v->rows[kv].parent != 0) continue;
            /* In pre-order a keyvalue's key is the row after it, and a
             * scalar key's value the row after that. */
            uint32_t key = kv + 1, value = kv + 2;
            if (v->rows[kv].kind != FIG_ROW_KEYVALUE || v->rows[key].kind != FIG_ROW_STRING)
                return fail_print(&b, err, "a dotenv key must be a string");
            size_t kn;
            const char *ks = text_of(v, v->rows[key].text, &kn);
            int ident = kn > 0 && is_key_start((uint8_t)ks[0]);
            for (size_t i = 1; i < kn; i++) ident = ident && is_key((uint8_t)ks[i]);
            if (!ident) return fail_print(&b, err, "a dotenv key is a bash identifier");
            if (!comment_lines(&b, v, key, FIG_COMMENT_LEADING) || !put(&b, ks, kn) || !put(&b, "=", 1)) return FIG_STATUS_OUT_OF_MEMORY;
            if (write_value(&b, v, value) <= 0) return fail_print(&b, err, "dotenv holds a flat map of strings");
            for (size_t i = 0; i < v->comment_count; i++) {
                const FigTableComment *c = &v->comments[i];
                if (c->node != value || c->slot != FIG_COMMENT_TRAILING) continue;
                size_t n;
                const char *s = text_of(v, c->text, &n);
                if (!put(&b, " #", 2)) return FIG_STATUS_OUT_OF_MEMORY;
                if (n > 0) {
                    if (!put(&b, " ", 1)) return FIG_STATUS_OUT_OF_MEMORY;
                    for (size_t j = 0; j < n; j++)
                        if (!put(&b, s[j] == '\n' ? " " : s + j, 1)) return FIG_STATUS_OUT_OF_MEMORY;
                }
                break;
            }
            if (!put(&b, "\n", 1)) return FIG_STATUS_OUT_OF_MEMORY;
        }
        if (!comment_lines(&b, v, 0, FIG_COMMENT_DANGLING)) return FIG_STATUS_OUT_OF_MEMORY;
    }
    out->ptr = (const uint8_t *)b.p;
    out->len = b.len;
    return 0;
}

static void free_bytes(void *ctx, FigStr bytes) {
    (void)ctx;
    free((void *)bytes.ptr);
}

/* Register the language as `name`; its format handle to *out_format. */
FigStatus kit_dotenv_c_register(const char *name, int *out_format, FigError *err) {
    /* Copied by fig_language_register, as the dialect and the samples are. */
    FigSyntax syntax;
    memset(&syntax, 0, sizeof syntax);
    syntax.size = sizeof syntax;
    syntax.comments.line.open = "#";
    syntax.comments.trailing.open = "#";
    syntax.kv_sep = "=";
    syntax.empty_map_literal = "{}";
    syntax.block_seq_editable = true;
    syntax.flow_containers = false;
    static const char *const extensions[] = { "env", NULL };
    FigDialectDesc dialect;
    memset(&dialect, 0, sizeof dialect);
    dialect.name = name;
    dialect.extensions = extensions;
    dialect.splice = 2; /* raw */
    dialect.empty_doc_seed = "";
    dialect.prints_flags = FIG_PRINTS_TEXT_SCALARS;
    dialect.prints_char_as = "number";
    static const char sample1[] = "A=1\nB=\"two words\"\n";
    static const char sample2[] = "# top\nexport C='raw \\n'\nD=x # trailing\n";
    const FigStr samples[] = { { (const uint8_t *)sample1, sizeof sample1 - 1 }, { (const uint8_t *)sample2, sizeof sample2 - 1 } };

    FigLanguageVTable vt;
    memset(&vt, 0, sizeof vt);
    vt.version = FIG_LANGUAGE_VTABLE_VERSION;
    vt.size = sizeof vt;
    vt.name = name;
    vt.caps = FIG_CAP_READ | FIG_CAP_EDIT | FIG_CAP_SERIALIZE;
    vt.max_mapping_depth = 0;
    vt.syntax = &syntax;
    vt.dialects = &dialect;
    vt.dialect_count = 1;
    vt.dialect_size = sizeof dialect;
    vt.samples = samples;
    vt.sample_count = 2;
    vt.parse = parse;
    vt.print = print;
    vt.free_bytes = free_bytes;
    return fig_language_register(&vt, out_format, err);
}
