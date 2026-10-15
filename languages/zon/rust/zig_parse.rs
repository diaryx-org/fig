//! Port of lib/std/zig/Parse.zig (Zig 0.17.0), as `std.zig.Ast.parse(gpa,
//! source, .{ .mode = .zon })` runs it.
//!
//! fig's ZON parser (`parser.rs`) hands the whole document to the Zig
//! standard library's parser in ZON mode — which parses ANY Zig expression,
//! leaving it to the caller to refuse what ZON does not allow — and calls the
//! document `InvalidZon` when the tree has errors, `UnsupportedZon` when it
//! parses but holds an expression ZON has no value for. This is that parser,
//! ported production for production, so that exactly the same documents
//! parse.
//!
//! **What is kept (Rust).** Zig's parser records an error and, in its
//! default recovering mode, carries on; any error at all makes the document
//! `InvalidZon`, so this port stops at the first (`Err(ParseError)`) —
//! every `warn` and `fail` of the Zig is a refusal here, and the recovery
//! paths (`findNextContainerMember`, `findNextStmt`) are never reached. The
//! tree it builds keeps what fig's walk reads: the ZON-shaped nodes (literals,
//! `-x`, `(x)`, `.{…}`/`T{…}` initializers) with their tokens, and for every
//! other node its first token and whether it is a variable declaration (the
//! one node tag the grammar itself consults).

use super::zig_tokenizer::{Tag, Token};

/// A node's index in `Ast::nodes`.
pub type NodeIndex = u32;
/// A token's index in `Ast::tokens`.
pub type TokenIndex = u32;

/// What the parse made of a node: the kinds fig's walk dispatches on, and
/// `Other` for every other Zig expression (an `UnsupportedZon`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    NumberLiteral,
    CharLiteral,
    Identifier,
    EnumLiteral,
    StringLiteral,
    /// Lines `main_token..=last_token`.
    MultilineStringLiteral,
    /// `-x`.
    Negation(NodeIndex),
    /// `(x)`.
    GroupedExpression(NodeIndex),
    /// `.{a, b}` (no type) or `T{a, b}`.
    ArrayInit { type_expr: Option<NodeIndex>, elements: Vec<NodeIndex> },
    /// `.{.a = x}` (no type), `T{.a = x}`, and the empty `.{}`/`T{}`. Each
    /// field is its value expression; its name is two tokens before the
    /// value's first.
    StructInit { type_expr: Option<NodeIndex>, fields: Vec<NodeIndex> },
    /// A `const`/`var` declaration.
    VarDecl,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    pub main_token: TokenIndex,
    /// `Ast.firstToken`: the token the node's source begins with.
    pub first_token: TokenIndex,
    /// `Ast.lastToken`: the token it ends with.
    pub last_token: TokenIndex,
}

/// A parsed ZON document (Zig's `Ast` in `.zon` mode, without errors).
#[derive(Debug, Clone)]
pub struct Ast {
    pub tokens: Vec<Token>,
    pub nodes: Vec<Node>,
    /// The document's one expression (`rootDecls()[0]`).
    pub root: NodeIndex,
}

impl Ast {
    pub fn node(&self, n: NodeIndex) -> &Node {
        &self.nodes[n as usize]
    }

    pub fn token_tag(&self, t: TokenIndex) -> Tag {
        self.tokens[t as usize].tag
    }

    pub fn token_start(&self, t: TokenIndex) -> usize {
        self.tokens[t as usize].start
    }

    /// `[start, end)` of token `t` in the source.
    pub fn token_end(&self, t: TokenIndex) -> usize {
        self.tokens[t as usize].end
    }
}

/// The parse found an error (Zig's `error.ParseError`, and any recorded
/// error of a recovering parse).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;

type R<T> = Result<T, ParseError>;

/// `Ast.parse(source, .{ .mode = .zon })`: the document, or `ParseError`
/// when the Zig parser would have recorded any error.
pub fn parse_zon(source: &[u8]) -> R<Ast> {
    let tokens = super::zig_tokenizer::tokenize(source);
    let mut p = Parse { source, tokens: &tokens, tok_i: 0, nodes: Vec::new() };
    let root = p.parse_zon()?;
    let nodes = std::mem::take(&mut p.nodes);
    Ok(Ast { tokens, nodes, root })
}

struct Parse<'a> {
    source: &'a [u8],
    tokens: &'a [Token],
    tok_i: TokenIndex,
    nodes: Vec<Node>,
}

/// A binary operator's precedence and associativity (`operTable`); `None`
/// for a token that is not one.
fn oper_info(tag: Tag) -> Option<(i32, bool)> {
    use Tag::*;
    // (precedence, associates: false for the comparison operators)
    Some(match tag {
        KeywordOr => (10, true),
        KeywordAnd => (20, true),
        EqualEqual | BangEqual | AngleBracketLeft | AngleBracketRight | AngleBracketLeftEqual
        | AngleBracketRightEqual => (30, false),
        Ampersand | Caret | Pipe | KeywordOrelse | KeywordCatch => (40, true),
        AngleBracketAngleBracketLeft | AngleBracketAngleBracketLeftPipe | AngleBracketAngleBracketRight => (50, true),
        Plus | Minus | PlusPlus | PlusPercent | MinusPercent | PlusPipe | MinusPipe => (60, true),
        PipePipe | Asterisk | Slash | Percent | AsteriskPercent | AsteriskPipe => (70, true),
        _ => return None,
    })
}

fn is_assign_op(tok: Tag) -> bool {
    use Tag::*;
    matches!(
        tok,
        AsteriskEqual
            | SlashEqual
            | PercentEqual
            | PlusEqual
            | MinusEqual
            | AngleBracketAngleBracketLeftEqual
            | AngleBracketAngleBracketLeftPipeEqual
            | AngleBracketAngleBracketRightEqual
            | AmpersandEqual
            | CaretEqual
            | PipeEqual
            | AsteriskPercentEqual
            | PlusPercentEqual
            | MinusPercentEqual
            | AsteriskPipeEqual
            | PlusPipeEqual
            | MinusPipeEqual
            | Equal
    )
}

/// `std.ascii.isWhitespace`.
fn is_ascii_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldState {
    None,
    Seen,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlignType {
    BitAlign,
    ByteAlign,
}

/// Which body an `if`/`for`/`while` takes (`bodyParseFn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    Expr,
    TypeExpr,
}

impl<'a> Parse<'a> {
    // ── Tokens ───────────────────────────────────────────────────────────

    /// The tag of token `i`; the token list always ends with `Eof`, which
    /// is what a lookahead past it reads.
    fn token_tag(&self, i: TokenIndex) -> Tag {
        self.tokens.get(i as usize).map_or(Tag::Eof, |t| t.tag)
    }

    fn token_start(&self, i: TokenIndex) -> usize {
        self.tokens[i as usize].start
    }

    fn next_token(&mut self) -> TokenIndex {
        let result = self.tok_i;
        self.tok_i += 1;
        result
    }

    fn eat_token(&mut self, tag: Tag) -> Option<TokenIndex> {
        if self.token_tag(self.tok_i) == tag { Some(self.next_token()) } else { None }
    }

    fn eat_tokens(&mut self, tags: &[Tag]) -> Option<TokenIndex> {
        let available = &self.tokens[self.tok_i as usize..];
        if available.len() < tags.len() || !available.iter().zip(tags).all(|(t, &tag)| t.tag == tag) {
            return None;
        }
        let result = self.tok_i;
        self.tok_i += tags.len() as u32;
        Some(result)
    }

    fn assert_token(&mut self, tag: Tag) -> TokenIndex {
        let token = self.next_token();
        debug_assert_eq!(tag, self.token_tag(token));
        token
    }

    fn expect_token(&mut self, tag: Tag) -> R<TokenIndex> {
        if self.token_tag(self.tok_i) != tag {
            return Err(ParseError);
        }
        Ok(self.next_token())
    }

    fn expect_semicolon(&mut self) -> R<()> {
        if self.token_tag(self.tok_i) == Tag::Semicolon {
            self.next_token();
            return Ok(());
        }
        Err(ParseError)
    }

    fn tokens_on_same_line(&self, token1: TokenIndex, token2: TokenIndex) -> bool {
        !self.source[self.token_start(token1)..self.token_start(token2)].contains(&b'\n')
    }

    // ── Nodes ────────────────────────────────────────────────────────────

    /// A node whose source runs from `first` to the last token consumed.
    fn add(&mut self, kind: Kind, main_token: TokenIndex, first_token: TokenIndex) -> NodeIndex {
        let i = self.nodes.len() as NodeIndex;
        self.nodes.push(Node { kind, main_token, first_token, last_token: self.tok_i.saturating_sub(1) });
        i
    }

    /// A node of a kind fig's walk never reads.
    fn other(&mut self, main_token: TokenIndex, first_token: TokenIndex) -> NodeIndex {
        self.add(Kind::Other, main_token, first_token)
    }

    fn first(&self, n: NodeIndex) -> TokenIndex {
        self.nodes[n as usize].first_token
    }

    fn is_var_decl(&self, n: NodeIndex) -> bool {
        self.nodes[n as usize].kind == Kind::VarDecl
    }

    // ── Roots ────────────────────────────────────────────────────────────

    /// Parse in ZON mode. Subset of the language.
    fn parse_zon(&mut self) -> R<NodeIndex> {
        let node_index = self.expect_expr()?;
        if self.token_tag(self.tok_i) != Tag::Eof {
            return Err(ParseError);
        }
        Ok(node_index)
    }

    fn parse_container_members(&mut self) -> R<()> {
        let mut field_state = FieldState::None;

        // Skip container doc comments.
        while self.eat_token(Tag::ContainerDocComment).is_some() {}

        loop {
            let doc_comment = self.eat_doc_comments()?;

            let mut tag = self.token_tag(self.tok_i);
            // `continue :sw` from the decl prong: `extern struct …` and
            // `inline for …` re-dispatch as their second token, which the
            // field prong takes.
            let redispatch = match tag {
                Tag::KeywordExtern => matches!(
                    self.token_tag(self.tok_i + 1),
                    Tag::KeywordStruct | Tag::KeywordUnion | Tag::KeywordEnum | Tag::KeywordOpaque
                ),
                Tag::KeywordInline => matches!(self.token_tag(self.tok_i + 1), Tag::KeywordFor | Tag::KeywordWhile),
                _ => false,
            };
            if redispatch {
                tag = self.token_tag(self.tok_i + 1);
            }
            match tag {
                Tag::KeywordTest => {
                    if doc_comment.is_some() {
                        return Err(ParseError); // test_doc_comment
                    }
                    let test_decl_node = self.expect_test_decl()?;
                    let _ = test_decl_node;
                    if field_state == FieldState::Seen {
                        field_state = FieldState::End;
                    }
                }
                Tag::KeywordComptime if self.token_tag(self.tok_i + 1) == Tag::LBrace => {
                    if doc_comment.is_some() {
                        return Err(ParseError); // comptime_doc_comment
                    }
                    self.next_token();
                    self.parse_block()?.ok_or(ParseError)?;
                    if field_state == FieldState::Seen {
                        field_state = FieldState::End;
                    }
                }
                Tag::KeywordComptime => {
                    if !self.container_field(&mut field_state)? {
                        break;
                    }
                }
                Tag::KeywordPub
                | Tag::KeywordConst
                | Tag::KeywordVar
                | Tag::KeywordThreadlocal
                | Tag::KeywordExport
                | Tag::KeywordExtern
                | Tag::KeywordInline
                | Tag::KeywordNoinline
                | Tag::KeywordFn => {
                    if tag == Tag::KeywordPub {
                        self.tok_i += 1;
                    }
                    self.expect_top_level_decl()?.ok_or(ParseError)?;
                    if field_state == FieldState::Seen {
                        field_state = FieldState::End;
                    }
                }
                Tag::Eof | Tag::RBrace => {
                    if doc_comment.is_some() {
                        return Err(ParseError); // unattached_doc_comment
                    }
                    break;
                }
                _ => {
                    if self.parse_c_style_container()? {
                        continue;
                    }
                    if !self.container_field(&mut field_state)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// A container field and what follows it: `true` to keep reading
    /// members, `false` at the container's end.
    fn container_field(&mut self, field_state: &mut FieldState) -> R<bool> {
        match *field_state {
            FieldState::None => *field_state = FieldState::Seen,
            FieldState::Seen => {}
            // decl_between_fields (the recovering parse's `.err` state
            // follows only an error)
            FieldState::End => return Err(ParseError),
        }
        self.expect_container_field()?;
        match self.token_tag(self.tok_i) {
            Tag::Comma => {
                self.tok_i += 1;
                Ok(true)
            }
            Tag::RBrace | Tag::Eof => Ok(false),
            // There is not allowed to be a decl after a field with no comma.
            _ => Err(ParseError), // expected_comma_after_field
        }
    }

    fn expect_test_decl(&mut self) -> R<NodeIndex> {
        let test_token = self.assert_token(Tag::KeywordTest);
        if matches!(self.token_tag(self.tok_i), Tag::StringLiteral | Tag::Identifier) {
            self.next_token();
        }
        self.parse_block()?.ok_or(ParseError)?; // expected_block
        Ok(self.other(test_token, test_token))
    }

    fn expect_top_level_decl(&mut self) -> R<Option<NodeIndex>> {
        let extern_export_inline_token = self.next_token();
        let mut is_extern = false;
        let mut expect_fn = false;
        let mut expect_var_or_fn = false;
        match self.token_tag(extern_export_inline_token) {
            Tag::KeywordExtern => {
                self.eat_token(Tag::StringLiteral);
                is_extern = true;
                expect_var_or_fn = true;
            }
            Tag::KeywordExport => expect_var_or_fn = true,
            Tag::KeywordInline | Tag::KeywordNoinline => expect_fn = true,
            _ => self.tok_i -= 1,
        }
        if let Some(fn_proto) = self.parse_fn_proto()? {
            match self.token_tag(self.tok_i) {
                Tag::Semicolon => {
                    self.tok_i += 1;
                    return Ok(Some(fn_proto));
                }
                Tag::LBrace => {
                    if is_extern {
                        return Err(ParseError); // extern_fn_body
                    }
                    self.parse_block()?;
                    let main = self.nodes[fn_proto as usize].main_token;
                    let first = self.first(fn_proto);
                    return Ok(Some(self.other(main, first)));
                }
                // Since parseBlock only returns ParseError on a missing '}'
                // we can assume this function was supposed to end here.
                _ => return Err(ParseError), // expected_semi_or_lbrace
            }
        }
        if expect_fn {
            return Err(ParseError); // expected_fn
        }

        let thread_local_token = self.eat_token(Tag::KeywordThreadlocal);
        if let Some(var_decl) = self.parse_global_var_decl()? {
            return Ok(Some(var_decl));
        }
        if thread_local_token.is_some() {
            return Err(ParseError); // expected_var_decl
        }
        if expect_var_or_fn {
            return Err(ParseError); // expected_var_decl_or_fn
        }
        Err(ParseError) // expected_pub_item
    }

    fn parse_fn_proto(&mut self) -> R<Option<NodeIndex>> {
        let Some(fn_token) = self.eat_token(Tag::KeywordFn) else { return Ok(None) };
        self.eat_token(Tag::Identifier);
        self.parse_param_decl_list()?;
        self.parse_byte_align()?;
        self.parse_addr_space()?;
        self.parse_link_section()?;
        self.parse_callconv()?;
        self.eat_token(Tag::Bang);

        let return_type_expr = self.parse_type_expr()?;
        if return_type_expr.is_none() {
            // most likely the user forgot to specify the return type.
            return Err(ParseError); // expected_return_type
        }
        Ok(Some(self.other(fn_token, fn_token)))
    }

    /// Returns a `*_var_decl` node with its init expression still to come.
    fn parse_var_decl_proto(&mut self) -> R<Option<NodeIndex>> {
        let Some(mut_token) = self.eat_token(Tag::KeywordConst).or_else(|| self.eat_token(Tag::KeywordVar)) else {
            return Ok(None);
        };

        self.expect_token(Tag::Identifier)?;
        if self.eat_token(Tag::Colon).is_some() {
            self.expect_type_expr()?;
        }
        self.parse_byte_align()?;
        self.parse_addr_space()?;
        self.parse_link_section()?;
        Ok(Some(self.add(Kind::VarDecl, mut_token, mut_token)))
    }

    fn parse_global_var_decl(&mut self) -> R<Option<NodeIndex>> {
        let Some(var_decl) = self.parse_var_decl_proto()? else { return Ok(None) };

        match self.token_tag(self.tok_i) {
            Tag::EqualEqual => return Err(ParseError), // wrong_equal_var_decl
            Tag::Equal => {
                self.tok_i += 1;
                self.expect_expr()?;
            }
            _ => {}
        }

        self.expect_semicolon()?; // expected_semi_after_decl
        Ok(Some(var_decl))
    }

    fn expect_container_field(&mut self) -> R<NodeIndex> {
        let first = self.tok_i;
        self.eat_token(Tag::KeywordComptime);
        let main_token = self.tok_i;
        self.eat_tokens(&[Tag::Identifier, Tag::Colon]);
        self.expect_type_expr()?;
        self.parse_byte_align()?;
        if self.eat_token(Tag::Equal).is_some() {
            self.expect_expr()?;
        }
        Ok(self.other(main_token, first))
    }

    // ── Statements ───────────────────────────────────────────────────────

    fn expect_statement(&mut self, is_block_level: bool) -> R<NodeIndex> {
        if let Some(comptime_token) = self.eat_token(Tag::KeywordComptime) {
            if self.parse_block_expr()?.is_some() {
                return Ok(self.other(comptime_token, comptime_token));
            }

            if is_block_level {
                return self.expect_var_assign_statement(Some(comptime_token));
            } else {
                self.expect_assign_expr()?;
                self.expect_semicolon()?; // expected_semi_after_stmt
                return Ok(self.other(comptime_token, comptime_token));
            }
        }

        let start = self.tok_i;
        match self.token_tag(self.tok_i) {
            Tag::KeywordNosuspend | Tag::KeywordSuspend => {
                let token = self.next_token();
                self.expect_block_expr_statement()?;
                return Ok(self.other(token, start));
            }
            Tag::KeywordDefer | Tag::KeywordErrdefer if is_block_level => {
                let token = self.next_token();
                self.expect_block_expr_statement()?;
                return Ok(self.other(token, start));
            }
            Tag::KeywordIf => return self.expect_if_statement(),
            Tag::KeywordEnum | Tag::KeywordStruct | Tag::KeywordUnion if self.parse_c_style_container()? => {
                unreachable!("a C-style container is always an error");
            }
            _ => {}
        }

        if let Some(labeled_statement) = self.parse_labeled_statement()? {
            return Ok(labeled_statement);
        }

        if is_block_level {
            self.expect_var_assign_statement(None)
        } else {
            let assign = self.expect_assign_expr()?;
            self.expect_semicolon()?; // expected_semi_after_stmt
            Ok(assign)
        }
    }

    fn expect_var_assign_statement(&mut self, comptime_token: Option<TokenIndex>) -> R<NodeIndex> {
        let start = comptime_token.unwrap_or(self.tok_i);
        let mut lhs: Vec<NodeIndex> = Vec::new();
        loop {
            if let Some(var_decl) = self.parse_var_decl_proto()? {
                lhs.push(var_decl);
            } else {
                // expected_statement / expected_expr_or_var_decl
                let expr = self.parse_expr()?.ok_or(ParseError)?;
                lhs.push(expr);
            }
            if self.eat_token(Tag::Comma).is_none() {
                break;
            }
        }

        let lhs_count = lhs.len();
        debug_assert!(lhs_count > 0);

        let equal_token = match self.eat_token(Tag::Equal) {
            Some(t) => t,
            None => {
                if lhs_count > 1 {
                    // Definitely a destructure; `==` is wrong_equal_var_decl.
                    return Err(ParseError);
                }
                if self.is_var_decl(lhs[0]) {
                    // Definitely a var decl; `==` is wrong_equal_var_decl.
                    return Err(ParseError);
                }
                let expr = self.finish_assign_expr(lhs[0])?;
                self.expect_semicolon()?; // expected_semi_after_stmt
                return Ok(match comptime_token {
                    Some(t) => self.other(t, t),
                    None => expr,
                });
            }
        };

        self.expect_expr()?;
        self.expect_semicolon()?; // expected_semi_after_stmt

        if lhs_count == 1 && self.is_var_decl(lhs[0]) {
            // Don't need to wrap in comptime
            return Ok(lhs[0]);
        }
        Ok(self.other(equal_token, start))
    }

    fn expect_if_statement(&mut self) -> R<NodeIndex> {
        let if_token = self.assert_token(Tag::KeywordIf);
        self.expect_token(Tag::LParen)?;
        self.expect_expr()?;
        self.expect_token(Tag::RParen)?;
        self.parse_ptr_payload()?;

        // TODO propose to change the syntax so that semicolons are always
        // required inside if statements, even if there is an `else`.
        let mut else_required = false;
        if self.parse_block_expr()?.is_none() {
            self.parse_assign_expr()?.ok_or(ParseError)?; // expected_block_or_assignment
            if self.eat_token(Tag::Semicolon).is_some() {
                return Ok(self.other(if_token, if_token));
            }
            else_required = true;
        }
        if self.eat_token(Tag::KeywordElse).is_none() {
            if else_required {
                return Err(ParseError); // expected_semi_or_else
            }
            return Ok(self.other(if_token, if_token));
        }
        self.parse_payload()?;
        self.expect_statement(false)?;
        Ok(self.other(if_token, if_token))
    }

    fn parse_labeled_statement(&mut self) -> R<Option<NodeIndex>> {
        let start = self.tok_i;
        let opt_label_token = self.parse_block_label();

        if let Some(block) = self.parse_block()? {
            self.nodes[block as usize].first_token = start;
            return Ok(Some(block));
        }
        if let Some(loop_stmt) = self.parse_loop_statement()? {
            self.nodes[loop_stmt as usize].first_token = start;
            return Ok(Some(loop_stmt));
        }
        if let Some(switch_expr) = self.parse_switch_expr(opt_label_token.is_some())? {
            self.nodes[switch_expr as usize].first_token = start;
            return Ok(Some(switch_expr));
        }

        if opt_label_token.is_none() {
            return Ok(None);
        }
        // expected_var_const / expected_labelable
        Err(ParseError)
    }

    fn parse_loop_statement(&mut self) -> R<Option<NodeIndex>> {
        let start = self.tok_i;
        let inline_token = self.eat_token(Tag::KeywordInline);

        if let Some(for_statement) = self.parse_for_statement(start)? {
            return Ok(Some(for_statement));
        }
        if let Some(while_statement) = self.parse_while_statement(start)? {
            return Ok(Some(while_statement));
        }

        if inline_token.is_none() {
            return Ok(None);
        }

        // If we've seen "inline", there should have been a "for" or "while"
        Err(ParseError) // expected_inlinable
    }

    fn parse_for_statement(&mut self, start: TokenIndex) -> R<Option<NodeIndex>> {
        let Some(for_token) = self.eat_token(Tag::KeywordFor) else { return Ok(None) };

        let inputs = self.for_prefix()?;

        let mut else_required = false;
        let mut seen_semicolon = false;
        if self.parse_block_expr()?.is_none() {
            self.parse_assign_expr()?.ok_or(ParseError)?; // expected_block_or_assignment
            if self.eat_token(Tag::Semicolon).is_some() {
                seen_semicolon = true;
            } else {
                else_required = true;
            }
        }
        if !seen_semicolon && self.eat_token(Tag::KeywordElse).is_some() {
            self.expect_statement(false)?;
        } else if inputs == 1 {
            if else_required {
                return Err(ParseError); // expected_semi_or_else
            }
        } else if else_required {
            return Err(ParseError); // expected_semi_or_else
        }
        Ok(Some(self.other(for_token, start)))
    }

    fn parse_while_statement(&mut self, start: TokenIndex) -> R<Option<NodeIndex>> {
        let Some(while_token) = self.eat_token(Tag::KeywordWhile) else { return Ok(None) };
        self.expect_token(Tag::LParen)?;
        self.expect_expr()?;
        self.expect_token(Tag::RParen)?;
        self.parse_ptr_payload()?;
        self.parse_while_continue_expr()?;

        // TODO propose to change the syntax so that semicolons are always
        // required inside while statements, even if there is an `else`.
        let mut else_required = false;
        if self.parse_block_expr()?.is_none() {
            self.parse_assign_expr()?.ok_or(ParseError)?; // expected_block_or_assignment
            if self.eat_token(Tag::Semicolon).is_some() {
                return Ok(Some(self.other(while_token, start)));
            }
            else_required = true;
        }
        if self.eat_token(Tag::KeywordElse).is_none() {
            if else_required {
                return Err(ParseError); // expected_semi_or_else
            }
            return Ok(Some(self.other(while_token, start)));
        }
        self.parse_payload()?;
        self.expect_statement(false)?;
        Ok(Some(self.other(while_token, start)))
    }

    fn parse_block_expr_statement(&mut self) -> R<Option<NodeIndex>> {
        if let Some(expr) = self.parse_block_expr()? {
            return Ok(Some(expr));
        }
        if let Some(expr) = self.parse_assign_expr()? {
            self.expect_semicolon()?; // expected_semi_after_stmt
            return Ok(Some(expr));
        }
        Ok(None)
    }

    fn expect_block_expr_statement(&mut self) -> R<NodeIndex> {
        self.parse_block_expr_statement()?.ok_or(ParseError) // expected_block_or_expr
    }

    fn parse_block_expr(&mut self) -> R<Option<NodeIndex>> {
        match self.token_tag(self.tok_i) {
            Tag::Identifier => {
                if self.token_tag(self.tok_i + 1) == Tag::Colon && self.token_tag(self.tok_i + 2) == Tag::LBrace {
                    let start = self.tok_i;
                    self.tok_i += 2;
                    let block = self.parse_block()?;
                    if let Some(b) = block {
                        self.nodes[b as usize].first_token = start;
                    }
                    Ok(block)
                } else {
                    Ok(None)
                }
            }
            Tag::LBrace => self.parse_block(),
            _ => Ok(None),
        }
    }

    // ── Assignment ───────────────────────────────────────────────────────

    fn parse_assign_expr(&mut self) -> R<Option<NodeIndex>> {
        let Some(expr) = self.parse_expr()? else { return Ok(None) };
        Ok(Some(self.finish_assign_expr(expr)?))
    }

    fn parse_single_assign_expr(&mut self) -> R<Option<NodeIndex>> {
        let Some(lhs) = self.parse_expr()? else { return Ok(None) };
        if !is_assign_op(self.token_tag(self.tok_i)) {
            return Ok(Some(lhs));
        }
        let main = self.next_token();
        self.expect_expr()?;
        let first = self.first(lhs);
        Ok(Some(self.other(main, first)))
    }

    fn finish_assign_expr(&mut self, lhs: NodeIndex) -> R<NodeIndex> {
        let tok = self.token_tag(self.tok_i);
        if tok == Tag::Comma {
            return self.finish_assign_destructure_expr(lhs);
        }
        if !is_assign_op(tok) {
            return Ok(lhs);
        }
        let main = self.next_token();
        self.expect_expr()?;
        let first = self.first(lhs);
        Ok(self.other(main, first))
    }

    fn finish_assign_destructure_expr(&mut self, first_lhs: NodeIndex) -> R<NodeIndex> {
        while self.eat_token(Tag::Comma).is_some() {
            self.expect_expr()?;
        }
        let equal_token = self.expect_token(Tag::Equal)?;
        self.expect_expr()?;
        let first = self.first(first_lhs);
        Ok(self.other(equal_token, first))
    }

    fn expect_single_assign_expr(&mut self) -> R<NodeIndex> {
        self.parse_single_assign_expr()?.ok_or(ParseError) // expected_expr_or_assignment
    }

    fn expect_assign_expr(&mut self) -> R<NodeIndex> {
        self.parse_assign_expr()?.ok_or(ParseError) // expected_expr_or_assignment
    }

    // ── Expressions ──────────────────────────────────────────────────────

    fn parse_expr(&mut self) -> R<Option<NodeIndex>> {
        self.parse_expr_precedence(0)
    }

    fn expect_expr(&mut self) -> R<NodeIndex> {
        self.parse_expr()?.ok_or(ParseError) // expected_expr
    }

    fn parse_expr_precedence(&mut self, min_prec: i32) -> R<Option<NodeIndex>> {
        debug_assert!(min_prec >= 0);
        let Some(mut node) = self.parse_prefix_expr()? else { return Ok(None) };

        let mut banned_prec: i32 = -1;

        loop {
            let tok_tag = self.token_tag(self.tok_i);
            let Some((prec, associates)) = oper_info(tok_tag) else { break };
            if prec < min_prec {
                break;
            }
            if prec == banned_prec {
                return Err(ParseError); // chained_comparison_operators
            }

            let oper_token = self.next_token();
            // Special-case handling for "catch"
            if tok_tag == Tag::KeywordCatch {
                self.parse_payload()?;
            }

            // Check for whitespace error before parsing the rhs.
            {
                let tok_len = tok_tag.lexeme().expect("an operator").len();
                let at = self.token_start(oper_token);
                if at + tok_len >= self.source.len() {
                    return Err(ParseError); // expected_expr
                }
                let char_before = self.source[at - 1];
                let char_after = self.source[at + tok_len];
                if tok_tag == Tag::Ampersand && char_after == b'&' {
                    return Err(ParseError); // invalid_ampersand_ampersand
                } else if is_ascii_whitespace(char_before) != is_ascii_whitespace(char_after) {
                    return Err(ParseError); // mismatched_binary_op_whitespace
                }
            }

            let Some(_rhs) = self.parse_expr_precedence(prec + 1)? else {
                return Err(ParseError); // expected_expr
            };

            let first = self.first(node);
            node = self.other(oper_token, first);

            if !associates {
                banned_prec = prec;
            }
        }

        Ok(Some(node))
    }

    fn parse_prefix_expr(&mut self) -> R<Option<NodeIndex>> {
        let negation = match self.token_tag(self.tok_i) {
            Tag::Minus => true,
            Tag::Bang | Tag::Tilde | Tag::MinusPercent | Tag::Ampersand | Tag::KeywordTry => false,
            _ => return self.parse_primary_expr(),
        };
        let main = self.next_token();
        let child = self.expect_prefix_expr()?;
        let kind = if negation { Kind::Negation(child) } else { Kind::Other };
        Ok(Some(self.add(kind, main, main)))
    }

    fn expect_prefix_expr(&mut self) -> R<NodeIndex> {
        self.parse_prefix_expr()?.ok_or(ParseError) // expected_prefix_expr
    }

    fn parse_type_expr(&mut self) -> R<Option<NodeIndex>> {
        let start = self.tok_i;
        match self.token_tag(self.tok_i) {
            Tag::QuestionMark => {
                let main = self.next_token();
                self.expect_type_expr()?;
                Ok(Some(self.other(main, start)))
            }
            Tag::KeywordAnyframe => match self.token_tag(self.tok_i + 1) {
                Tag::Arrow => {
                    let main = self.next_token();
                    self.next_token();
                    self.expect_type_expr()?;
                    Ok(Some(self.other(main, start)))
                }
                _ => self.parse_error_union_expr(),
            },
            Tag::Asterisk => {
                let asterisk = self.next_token();
                self.parse_ptr_modifiers(AlignType::BitAlign)?;
                self.expect_type_expr()?;
                Ok(Some(self.other(asterisk, start)))
            }
            Tag::LBracket => match self.token_tag(self.tok_i + 1) {
                Tag::Asterisk => {
                    let l_bracket = self.next_token();
                    self.next_token();
                    if let Some(ident) = self.eat_token(Tag::Identifier) {
                        let ident_slice = &self.source[self.token_start(ident)..self.token_start(ident + 1)];
                        let end = ident_slice.iter().rposition(|&c| !is_ascii_whitespace(c)).map_or(0, |e| e + 1);
                        if &ident_slice[..end] != b"c" {
                            self.tok_i -= 1;
                        }
                    } else if self.eat_token(Tag::Colon).is_some() {
                        self.expect_expr()?;
                    }
                    self.expect_token(Tag::RBracket)?;
                    self.parse_ptr_modifiers(AlignType::ByteAlign)?;
                    self.expect_type_expr()?;
                    Ok(Some(self.other(l_bracket, start)))
                }
                _ => {
                    let lbracket = self.next_token();
                    let len_expr = self.parse_expr()?;
                    if self.eat_token(Tag::Colon).is_some() {
                        self.expect_expr()?;
                    }
                    self.expect_token(Tag::RBracket)?;
                    if len_expr.is_none() {
                        self.parse_ptr_modifiers(AlignType::ByteAlign)?;
                        self.expect_type_expr()?;
                    } else {
                        match self.token_tag(self.tok_i) {
                            Tag::KeywordAlign
                            | Tag::KeywordConst
                            | Tag::KeywordVolatile
                            | Tag::KeywordAllowzero
                            | Tag::KeywordAddrspace => return Err(ParseError), // ptr_mod_on_array_child_type
                            _ => {}
                        }
                        self.expect_type_expr()?;
                    }
                    Ok(Some(self.other(lbracket, start)))
                }
            },
            _ => self.parse_error_union_expr(),
        }
    }

    fn expect_type_expr(&mut self) -> R<NodeIndex> {
        self.parse_type_expr()?.ok_or(ParseError) // expected_type_expr
    }

    fn parse_primary_expr(&mut self) -> R<Option<NodeIndex>> {
        let start = self.tok_i;
        match self.token_tag(self.tok_i) {
            Tag::KeywordAsm => Ok(Some(self.expect_asm_expr()?)),
            Tag::KeywordIf => Ok(Some(self.expect_if(Body::Expr)?)),
            Tag::KeywordBreak | Tag::KeywordContinue => {
                let main = self.next_token();
                self.parse_break_label();
                self.parse_expr()?;
                Ok(Some(self.other(main, start)))
            }
            Tag::KeywordComptime | Tag::KeywordNosuspend | Tag::KeywordResume => {
                let main = self.next_token();
                self.expect_expr()?;
                Ok(Some(self.other(main, start)))
            }
            Tag::KeywordReturn => {
                let main = self.next_token();
                self.parse_expr()?;
                Ok(Some(self.other(main, start)))
            }
            Tag::Identifier => {
                if self.token_tag(self.tok_i + 1) == Tag::Colon {
                    match self.token_tag(self.tok_i + 2) {
                        Tag::KeywordInline => {
                            self.tok_i += 3;
                            match self.token_tag(self.tok_i) {
                                Tag::KeywordFor => Ok(Some(self.expect_for(Body::Expr, start)?)),
                                Tag::KeywordWhile => Ok(Some(self.expect_while_expr(start)?)),
                                _ => Err(ParseError), // expected_inlinable
                            }
                        }
                        Tag::KeywordFor => {
                            self.tok_i += 2;
                            Ok(Some(self.expect_for(Body::Expr, start)?))
                        }
                        Tag::KeywordWhile => {
                            self.tok_i += 2;
                            Ok(Some(self.expect_while_expr(start)?))
                        }
                        _ => self.parse_curly_suffix_expr(),
                    }
                } else {
                    self.parse_curly_suffix_expr()
                }
            }
            Tag::KeywordInline => {
                self.tok_i += 1;
                match self.token_tag(self.tok_i) {
                    Tag::KeywordFor => Ok(Some(self.expect_for(Body::Expr, start)?)),
                    Tag::KeywordWhile => Ok(Some(self.expect_while_expr(start)?)),
                    _ => Err(ParseError), // expected_inlinable
                }
            }
            Tag::KeywordFor => Ok(Some(self.expect_for(Body::Expr, start)?)),
            Tag::KeywordWhile => Ok(Some(self.expect_while_expr(start)?)),
            Tag::LBrace => self.parse_block(),
            _ => self.parse_curly_suffix_expr(),
        }
    }

    fn parse_block(&mut self) -> R<Option<NodeIndex>> {
        let Some(lbrace) = self.eat_token(Tag::LBrace) else { return Ok(None) };
        loop {
            if self.token_tag(self.tok_i) == Tag::RBrace {
                break;
            }
            self.expect_statement(true)?;
        }
        self.expect_token(Tag::RBrace)?;
        Ok(Some(self.other(lbrace, lbrace)))
    }

    fn for_prefix(&mut self) -> R<usize> {
        let mut inputs = 0usize;
        self.expect_token(Tag::LParen)?;

        loop {
            self.expect_expr()?;
            if self.eat_token(Tag::Ellipsis2).is_some() {
                self.parse_expr()?;
            }

            inputs += 1;
            match self.token_tag(self.tok_i) {
                Tag::Comma => self.tok_i += 1,
                Tag::RParen => {
                    self.tok_i += 1;
                    break;
                }
                Tag::Colon | Tag::RBrace | Tag::RBracket => return Err(ParseError), // expected r_paren
                // Likely just a missing comma.
                _ => return Err(ParseError), // expected_comma_after_for_operand
            }
            if self.eat_token(Tag::RParen).is_some() {
                break;
            }
        }

        if self.eat_token(Tag::Pipe).is_none() {
            return Err(ParseError); // expected_loop_payload
        }

        loop {
            self.eat_token(Tag::Asterisk);
            self.expect_token(Tag::Identifier)?;
            match self.token_tag(self.tok_i) {
                Tag::Comma => self.tok_i += 1,
                Tag::Pipe => {
                    self.tok_i += 1;
                    break;
                }
                // Likely just a missing comma.
                _ => return Err(ParseError), // expected_comma_after_capture
            }
            if self.eat_token(Tag::Pipe).is_some() {
                break;
            }
        }

        Ok(inputs)
    }

    fn expect_while_expr(&mut self, start: TokenIndex) -> R<NodeIndex> {
        let while_token = self.assert_token(Tag::KeywordWhile);
        self.expect_token(Tag::LParen)?;
        self.expect_expr()?;
        self.expect_token(Tag::RParen)?;
        self.parse_ptr_payload()?;
        self.parse_while_continue_expr()?;

        self.expect_expr()?;
        if self.eat_token(Tag::KeywordElse).is_some() {
            self.parse_payload()?;
            self.expect_expr()?;
        }
        Ok(self.other(while_token, start))
    }

    fn parse_curly_suffix_expr(&mut self) -> R<Option<NodeIndex>> {
        let Some(lhs) = self.parse_type_expr()? else { return Ok(None) };
        let Some(lbrace) = self.eat_token(Tag::LBrace) else { return Ok(Some(lhs)) };
        let first = self.first(lhs);

        if let Some(field_init) = self.parse_field_init()? {
            let mut inits = vec![field_init];
            loop {
                match self.token_tag(self.tok_i) {
                    Tag::Comma => self.tok_i += 1,
                    Tag::RBrace => {
                        self.tok_i += 1;
                        break;
                    }
                    Tag::Colon | Tag::RParen | Tag::RBracket => return Err(ParseError), // expected r_brace
                    // Likely just a missing comma.
                    _ => return Err(ParseError), // expected_comma_after_initializer
                }
                if self.eat_token(Tag::RBrace).is_some() {
                    break;
                }
                inits.push(self.expect_field_init()?);
            }
            return Ok(Some(self.add(Kind::StructInit { type_expr: Some(lhs), fields: inits }, lbrace, first)));
        }

        let mut inits: Vec<NodeIndex> = Vec::new();
        loop {
            if self.eat_token(Tag::RBrace).is_some() {
                break;
            }
            inits.push(self.expect_expr()?);
            match self.token_tag(self.tok_i) {
                Tag::Comma => self.tok_i += 1,
                Tag::RBrace => {
                    self.tok_i += 1;
                    break;
                }
                Tag::Colon | Tag::RParen | Tag::RBracket => return Err(ParseError), // expected r_brace
                // Likely just a missing comma.
                _ => return Err(ParseError), // expected_comma_after_initializer
            }
        }
        let kind = if inits.is_empty() {
            Kind::StructInit { type_expr: Some(lhs), fields: Vec::new() }
        } else {
            Kind::ArrayInit { type_expr: Some(lhs), elements: inits }
        };
        Ok(Some(self.add(kind, lbrace, first)))
    }

    fn parse_error_union_expr(&mut self) -> R<Option<NodeIndex>> {
        let Some(suffix_expr) = self.parse_suffix_expr()? else { return Ok(None) };
        let Some(bang) = self.eat_token(Tag::Bang) else { return Ok(Some(suffix_expr)) };
        self.expect_type_expr()?;
        let first = self.first(suffix_expr);
        Ok(Some(self.other(bang, first)))
    }

    fn parse_suffix_expr(&mut self) -> R<Option<NodeIndex>> {
        let Some(mut res) = self.parse_primary_type_expr()? else { return Ok(None) };
        while let Some(suffix_op) = self.parse_suffix_op(res)? {
            res = suffix_op;
        }
        Ok(Some(res))
    }

    fn parse_primary_type_expr(&mut self) -> R<Option<NodeIndex>> {
        let start = self.tok_i;
        match self.token_tag(self.tok_i) {
            Tag::CharLiteral => {
                let main = self.next_token();
                Ok(Some(self.add(Kind::CharLiteral, main, main)))
            }
            Tag::NumberLiteral => {
                let main = self.next_token();
                Ok(Some(self.add(Kind::NumberLiteral, main, main)))
            }
            Tag::KeywordUnreachable | Tag::KeywordAnyframe => {
                let main = self.next_token();
                Ok(Some(self.other(main, main)))
            }
            Tag::StringLiteral => {
                let main = self.next_token();
                Ok(Some(self.add(Kind::StringLiteral, main, main)))
            }

            Tag::Builtin => Ok(Some(self.expect_builtin_call()?)),
            Tag::KeywordFn => Ok(Some(self.parse_fn_proto()?.expect("a fn proto"))),
            Tag::KeywordIf => Ok(Some(self.expect_if(Body::TypeExpr)?)),
            Tag::KeywordSwitch => Ok(Some(self.expect_switch_expr(start)?)),

            Tag::KeywordExtern | Tag::KeywordPacked => {
                self.tok_i += 1;
                Ok(Some(self.expect_container_decl_auto(start)?))
            }

            Tag::KeywordStruct | Tag::KeywordOpaque | Tag::KeywordEnum | Tag::KeywordUnion => {
                Ok(Some(self.expect_container_decl_auto(start)?))
            }

            Tag::KeywordComptime => {
                let main = self.next_token();
                self.expect_type_expr()?;
                Ok(Some(self.other(main, start)))
            }
            Tag::MultilineStringLiteralLine => {
                let first_line = self.next_token();
                while self.token_tag(self.tok_i) == Tag::MultilineStringLiteralLine {
                    self.tok_i += 1;
                }
                Ok(Some(self.add(Kind::MultilineStringLiteral, first_line, first_line)))
            }
            Tag::Identifier => match self.token_tag(self.tok_i + 1) {
                Tag::Colon => match self.token_tag(self.tok_i + 2) {
                    Tag::KeywordInline => {
                        self.tok_i += 3;
                        match self.token_tag(self.tok_i) {
                            Tag::KeywordFor => Ok(Some(self.expect_for(Body::TypeExpr, start)?)),
                            Tag::KeywordWhile => Ok(Some(self.expect_while_type_expr(start)?)),
                            _ => Err(ParseError), // expected_inlinable
                        }
                    }
                    Tag::KeywordFor => {
                        self.tok_i += 2;
                        Ok(Some(self.expect_for(Body::TypeExpr, start)?))
                    }
                    Tag::KeywordWhile => {
                        self.tok_i += 2;
                        Ok(Some(self.expect_while_type_expr(start)?))
                    }
                    Tag::KeywordSwitch => {
                        self.tok_i += 2;
                        Ok(Some(self.expect_switch_expr(start)?))
                    }
                    Tag::LBrace => {
                        self.tok_i += 2;
                        let block = self.parse_block()?.expect("a block");
                        self.nodes[block as usize].first_token = start;
                        Ok(Some(block))
                    }
                    _ => {
                        let main = self.next_token();
                        Ok(Some(self.add(Kind::Identifier, main, main)))
                    }
                },
                _ => {
                    let main = self.next_token();
                    Ok(Some(self.add(Kind::Identifier, main, main)))
                }
            },
            Tag::KeywordInline => {
                self.tok_i += 1;
                match self.token_tag(self.tok_i) {
                    Tag::KeywordFor => Ok(Some(self.expect_for(Body::TypeExpr, start)?)),
                    Tag::KeywordWhile => Ok(Some(self.expect_while_type_expr(start)?)),
                    _ => Err(ParseError), // expected_inlinable
                }
            }
            Tag::KeywordFor => Ok(Some(self.expect_for(Body::TypeExpr, start)?)),
            Tag::KeywordWhile => Ok(Some(self.expect_while_type_expr(start)?)),
            Tag::Period => match self.token_tag(self.tok_i + 1) {
                Tag::Identifier => {
                    self.tok_i += 1;
                    let main = self.next_token(); // identifier
                    Ok(Some(self.add(Kind::EnumLiteral, main, start)))
                }
                Tag::LBrace => {
                    let lbrace = self.tok_i + 1;
                    self.tok_i = lbrace + 1;

                    if let Some(field_init) = self.parse_field_init()? {
                        let mut inits = vec![field_init];
                        loop {
                            match self.token_tag(self.tok_i) {
                                Tag::Comma => self.tok_i += 1,
                                Tag::RBrace => {
                                    self.tok_i += 1;
                                    break;
                                }
                                Tag::Colon | Tag::RParen | Tag::RBracket => return Err(ParseError), // expected r_brace
                                // Likely just a missing comma.
                                _ => return Err(ParseError), // expected_comma_after_initializer
                            }
                            if self.eat_token(Tag::RBrace).is_some() {
                                break;
                            }
                            inits.push(self.expect_field_init()?);
                        }
                        return Ok(Some(self.add(Kind::StructInit { type_expr: None, fields: inits }, lbrace, start)));
                    }

                    let mut inits: Vec<NodeIndex> = Vec::new();
                    loop {
                        if self.eat_token(Tag::RBrace).is_some() {
                            break;
                        }
                        inits.push(self.expect_expr()?);
                        match self.token_tag(self.tok_i) {
                            Tag::Comma => self.tok_i += 1,
                            Tag::RBrace => {
                                self.tok_i += 1;
                                break;
                            }
                            Tag::Colon | Tag::RParen | Tag::RBracket => return Err(ParseError), // expected r_brace
                            // Likely just a missing comma.
                            _ => return Err(ParseError), // expected_comma_after_initializer
                        }
                    }
                    let kind = if inits.is_empty() {
                        Kind::StructInit { type_expr: None, fields: Vec::new() }
                    } else {
                        Kind::ArrayInit { type_expr: None, elements: inits }
                    };
                    Ok(Some(self.add(kind, lbrace, start)))
                }
                _ => Ok(None),
            },
            Tag::KeywordError => match self.token_tag(self.tok_i + 1) {
                Tag::LBrace => {
                    let error_token = self.tok_i;
                    self.tok_i += 2;
                    loop {
                        if self.eat_token(Tag::RBrace).is_some() {
                            break;
                        }
                        self.eat_doc_comments()?;
                        self.expect_token(Tag::Identifier)?;
                        match self.token_tag(self.tok_i) {
                            Tag::Comma => self.tok_i += 1,
                            Tag::RBrace => {
                                self.tok_i += 1;
                                break;
                            }
                            Tag::Colon | Tag::RParen | Tag::RBracket => return Err(ParseError), // expected r_brace
                            // Likely just a missing comma.
                            _ => return Err(ParseError), // expected_comma_after_field
                        }
                    }
                    Ok(Some(self.other(error_token, start)))
                }
                _ => {
                    let main = self.next_token();
                    self.expect_token(Tag::Period)?;
                    self.expect_token(Tag::Identifier)?;
                    Ok(Some(self.other(main, start)))
                }
            },
            Tag::LParen => {
                let main = self.next_token();
                let child = self.expect_expr()?;
                self.expect_token(Tag::RParen)?;
                Ok(Some(self.add(Kind::GroupedExpression(child), main, start)))
            }
            _ => Ok(None),
        }
    }

    fn expect_while_type_expr(&mut self, start: TokenIndex) -> R<NodeIndex> {
        let while_token = self.assert_token(Tag::KeywordWhile);
        self.expect_token(Tag::LParen)?;
        self.expect_expr()?;
        self.expect_token(Tag::RParen)?;
        self.parse_ptr_payload()?;
        self.parse_while_continue_expr()?;

        self.expect_type_expr()?;
        if self.eat_token(Tag::KeywordElse).is_some() {
            self.parse_payload()?;
            self.expect_type_expr()?;
        }
        Ok(self.other(while_token, start))
    }

    fn parse_switch_expr(&mut self, _is_labeled: bool) -> R<Option<NodeIndex>> {
        let start = self.tok_i;
        let Some(_switch_token) = self.eat_token(Tag::KeywordSwitch) else { return Ok(None) };
        Ok(Some(self.expect_switch_suffix(start)?))
    }

    fn expect_switch_expr(&mut self, start: TokenIndex) -> R<NodeIndex> {
        self.assert_token(Tag::KeywordSwitch);
        self.expect_switch_suffix(start)
    }

    fn expect_switch_suffix(&mut self, start: TokenIndex) -> R<NodeIndex> {
        self.expect_token(Tag::LParen)?;
        self.expect_expr()?;
        self.expect_token(Tag::RParen)?;
        self.expect_token(Tag::LBrace)?;
        self.parse_switch_prong_list()?;
        self.expect_token(Tag::RBrace)?;
        Ok(self.other(start, start))
    }

    fn expect_asm_expr(&mut self) -> R<NodeIndex> {
        let asm_token = self.assert_token(Tag::KeywordAsm);
        self.eat_token(Tag::KeywordVolatile);
        self.expect_token(Tag::LParen)?;
        self.expect_expr()?;

        if self.eat_token(Tag::RParen).is_some() {
            return Ok(self.other(asm_token, asm_token));
        }

        self.expect_token(Tag::Colon)?;

        loop {
            if self.parse_asm_output_item()?.is_none() {
                break;
            }
            match self.token_tag(self.tok_i) {
                Tag::Comma => self.tok_i += 1,
                // All possible delimiters.
                Tag::Colon | Tag::RParen | Tag::RBrace | Tag::RBracket => break,
                // Likely just a missing comma.
                _ => return Err(ParseError), // expected comma
            }
        }

        if self.eat_token(Tag::Colon).is_some() {
            loop {
                if self.parse_asm_input_item()?.is_none() {
                    break;
                }
                match self.token_tag(self.tok_i) {
                    Tag::Comma => self.tok_i += 1,
                    // All possible delimiters.
                    Tag::Colon | Tag::RParen | Tag::RBrace | Tag::RBracket => break,
                    // Likely just a missing comma.
                    _ => return Err(ParseError), // expected comma
                }
            }

            if self.eat_token(Tag::Colon).is_some() {
                self.expect_expr()?;
            }
        }

        self.expect_token(Tag::RParen)?;
        Ok(self.other(asm_token, asm_token))
    }

    fn parse_asm_output_item(&mut self) -> R<Option<NodeIndex>> {
        if self.eat_token(Tag::LBracket).is_none() {
            return Ok(None);
        }
        let identifier = self.expect_token(Tag::Identifier)?;
        self.expect_token(Tag::RBracket)?;
        self.expect_token(Tag::StringLiteral)?;
        self.expect_token(Tag::LParen)?;
        if self.eat_token(Tag::Arrow).is_some() {
            self.expect_type_expr()?;
        } else {
            self.expect_token(Tag::Identifier)?;
        }
        self.expect_token(Tag::RParen)?;
        Ok(Some(self.other(identifier, identifier - 1)))
    }

    fn parse_asm_input_item(&mut self) -> R<Option<NodeIndex>> {
        if self.eat_token(Tag::LBracket).is_none() {
            return Ok(None);
        }
        let identifier = self.expect_token(Tag::Identifier)?;
        self.expect_token(Tag::RBracket)?;
        self.expect_token(Tag::StringLiteral)?;
        self.expect_token(Tag::LParen)?;
        self.expect_expr()?;
        self.expect_token(Tag::RParen)?;
        Ok(Some(self.other(identifier, identifier - 1)))
    }

    fn parse_break_label(&mut self) -> Option<TokenIndex> {
        self.eat_tokens(&[Tag::Colon, Tag::Identifier]).map(|i| i + 1)
    }

    fn parse_block_label(&mut self) -> Option<TokenIndex> {
        self.eat_tokens(&[Tag::Identifier, Tag::Colon])
    }

    fn parse_field_init(&mut self) -> R<Option<NodeIndex>> {
        if self.eat_tokens(&[Tag::Period, Tag::Identifier, Tag::Equal]).is_some() {
            return Ok(Some(self.expect_expr()?));
        }
        Ok(None)
    }

    fn expect_field_init(&mut self) -> R<NodeIndex> {
        if self.eat_tokens(&[Tag::Period, Tag::Identifier, Tag::Equal]).is_some() {
            return self.expect_expr();
        }
        Err(ParseError) // expected_initializer
    }

    fn parse_while_continue_expr(&mut self) -> R<Option<NodeIndex>> {
        if self.eat_token(Tag::Colon).is_none() {
            return Ok(None);
        }
        self.expect_token(Tag::LParen)?;
        let node = self.parse_assign_expr()?.ok_or(ParseError)?; // expected_expr_or_assignment
        self.expect_token(Tag::RParen)?;
        Ok(Some(node))
    }

    fn parse_link_section(&mut self) -> R<Option<NodeIndex>> {
        self.parse_parenthesized(Tag::KeywordLinksection)
    }

    fn parse_callconv(&mut self) -> R<Option<NodeIndex>> {
        self.parse_parenthesized(Tag::KeywordCallconv)
    }

    fn parse_addr_space(&mut self) -> R<Option<NodeIndex>> {
        self.parse_parenthesized(Tag::KeywordAddrspace)
    }

    fn parse_byte_align(&mut self) -> R<Option<NodeIndex>> {
        self.parse_parenthesized(Tag::KeywordAlign)
    }

    /// `keyword ( expr )`, the shape of `linksection`, `callconv`,
    /// `addrspace` and `align`.
    fn parse_parenthesized(&mut self, keyword: Tag) -> R<Option<NodeIndex>> {
        if self.eat_token(keyword).is_none() {
            return Ok(None);
        }
        self.expect_token(Tag::LParen)?;
        let expr_node = self.expect_expr()?;
        self.expect_token(Tag::RParen)?;
        Ok(Some(expr_node))
    }

    /// This function can return `None` and then still return nodes
    /// afterwards, such as in the case of anytype and `...`. Caller must
    /// look for rparen to find out when there are no more param decls left.
    fn expect_param_decl(&mut self) -> R<Option<NodeIndex>> {
        self.eat_doc_comments()?;
        match self.token_tag(self.tok_i) {
            Tag::KeywordNoalias | Tag::KeywordComptime => self.tok_i += 1,
            Tag::Ellipsis3 => {
                self.tok_i += 1;
                return Ok(None);
            }
            _ => {}
        }
        self.eat_tokens(&[Tag::Identifier, Tag::Colon]);
        if self.eat_token(Tag::KeywordAnytype).is_some() {
            Ok(None)
        } else {
            Ok(Some(self.expect_type_expr()?))
        }
    }

    fn parse_payload(&mut self) -> R<Option<TokenIndex>> {
        if self.eat_token(Tag::Pipe).is_none() {
            return Ok(None);
        }
        let identifier = self.expect_token(Tag::Identifier)?;
        self.expect_token(Tag::Pipe)?;
        Ok(Some(identifier))
    }

    fn parse_ptr_payload(&mut self) -> R<Option<TokenIndex>> {
        if self.eat_token(Tag::Pipe).is_none() {
            return Ok(None);
        }
        self.eat_token(Tag::Asterisk);
        let identifier = self.expect_token(Tag::Identifier)?;
        self.expect_token(Tag::Pipe)?;
        Ok(Some(identifier))
    }

    /// Returns the first identifier token, if any.
    fn parse_ptr_index_payload(&mut self) -> R<Option<TokenIndex>> {
        if self.eat_token(Tag::Pipe).is_none() {
            return Ok(None);
        }
        self.eat_token(Tag::Asterisk);
        let identifier = self.expect_token(Tag::Identifier)?;
        if self.eat_token(Tag::Comma).is_some() {
            self.expect_token(Tag::Identifier)?;
        }
        self.expect_token(Tag::Pipe)?;
        Ok(Some(identifier))
    }

    fn parse_switch_prong(&mut self) -> R<Option<NodeIndex>> {
        let is_inline = self.eat_token(Tag::KeywordInline).is_some();

        if self.eat_token(Tag::KeywordElse).is_none() {
            let mut items = 0usize;
            loop {
                if self.parse_switch_item()?.is_none() {
                    break;
                }
                items += 1;
                if self.eat_token(Tag::Comma).is_none() {
                    break;
                }
            }
            if items == 0 {
                if is_inline {
                    self.tok_i -= 1;
                }
                return Ok(None);
            }
        }
        let arrow_token = self.expect_token(Tag::EqualAngleBracketRight)?;
        self.parse_ptr_index_payload()?;
        self.expect_single_assign_expr()?;
        Ok(Some(self.other(arrow_token, arrow_token)))
    }

    fn parse_switch_item(&mut self) -> R<Option<NodeIndex>> {
        let Some(expr) = self.parse_expr()? else { return Ok(None) };

        if let Some(token) = self.eat_token(Tag::Ellipsis3) {
            self.expect_expr()?;
            let first = self.first(expr);
            return Ok(Some(self.other(token, first)));
        }
        Ok(Some(expr))
    }

    fn parse_ptr_modifiers(&mut self, align_type: AlignType) -> R<()> {
        let mut has_align = false;
        let mut has_addrspace = false;
        loop {
            match self.token_tag(self.tok_i) {
                Tag::KeywordAlign => {
                    if has_align {
                        return Err(ParseError); // extra_align_qualifier
                    }
                    self.tok_i += 1;
                    self.expect_token(Tag::LParen)?;
                    self.expect_expr()?;
                    has_align = true;

                    if self.eat_token(Tag::Colon).is_some() {
                        if align_type == AlignType::ByteAlign {
                            return Err(ParseError); // invalid_bit_range
                        }
                        self.expect_expr()?;
                        self.expect_token(Tag::Colon)?;
                        self.expect_expr()?;
                    }

                    self.expect_token(Tag::RParen)?;
                }
                Tag::KeywordAddrspace => {
                    if has_addrspace {
                        return Err(ParseError); // extra_addrspace_qualifier
                    }
                    has_addrspace = self.parse_addr_space()?.is_some();
                }
                Tag::KeywordAllowzero | Tag::KeywordConst | Tag::KeywordVolatile => self.tok_i += 1,
                _ => return Ok(()),
            }
        }
    }

    fn parse_suffix_op(&mut self, lhs: NodeIndex) -> R<Option<NodeIndex>> {
        let first = self.first(lhs);
        match self.token_tag(self.tok_i) {
            Tag::LBracket => {
                let lbracket = self.next_token();
                self.expect_expr()?;

                if self.eat_token(Tag::Ellipsis2).is_some() {
                    self.parse_expr()?;
                    if self.eat_token(Tag::Colon).is_some() {
                        self.expect_expr()?;
                    }
                }
                self.expect_token(Tag::RBracket)?;
                Ok(Some(self.other(lbracket, first)))
            }
            Tag::PeriodAsterisk => {
                let main = self.next_token();
                Ok(Some(self.other(main, first)))
            }
            Tag::Period => match self.token_tag(self.tok_i + 1) {
                Tag::Identifier | Tag::QuestionMark => {
                    let main = self.next_token();
                    self.next_token();
                    Ok(Some(self.other(main, first)))
                }
                // this a misplaced `.{`, handle the error somewhere else
                Tag::LBrace => Ok(None),
                _ => Err(ParseError), // expected_suffix_op
            },
            Tag::LParen => {
                let lparen = self.next_token();
                loop {
                    if self.eat_token(Tag::RParen).is_some() {
                        break;
                    }
                    self.expect_expr()?;
                    match self.token_tag(self.tok_i) {
                        Tag::Comma => self.tok_i += 1,
                        Tag::RParen => {
                            self.tok_i += 1;
                            break;
                        }
                        Tag::Colon | Tag::RBrace | Tag::RBracket => return Err(ParseError), // expected r_paren
                        // Likely just a missing comma.
                        _ => return Err(ParseError), // expected_comma_after_arg
                    }
                }
                Ok(Some(self.other(lparen, first)))
            }
            _ => Ok(None),
        }
    }

    /// Caller must have already verified the first token.
    fn expect_container_decl_auto(&mut self, start: TokenIndex) -> R<NodeIndex> {
        let main_token = self.next_token();
        match self.token_tag(main_token) {
            Tag::KeywordOpaque => {}
            Tag::KeywordStruct | Tag::KeywordEnum => {
                if self.eat_token(Tag::LParen).is_some() {
                    self.expect_expr()?;
                    self.expect_token(Tag::RParen)?;
                }
            }
            Tag::KeywordUnion => {
                if self.eat_token(Tag::LParen).is_some() {
                    if self.eat_token(Tag::KeywordEnum).is_some() {
                        if self.eat_token(Tag::LParen).is_some() {
                            self.expect_expr()?;
                            self.expect_token(Tag::RParen)?;
                            self.expect_token(Tag::RParen)?;
                        } else {
                            self.expect_token(Tag::RParen)?;
                        }
                    } else {
                        self.expect_expr()?;
                        self.expect_token(Tag::RParen)?;
                    }
                }
            }
            _ => {
                self.tok_i -= 1;
                return Err(ParseError); // expected_container
            }
        }
        self.expect_token(Tag::LBrace)?;
        self.parse_container_members()?;
        self.expect_token(Tag::RBrace)?;
        Ok(self.other(main_token, start))
    }

    /// Give a helpful error message for those transitioning from C's
    /// `struct Foo {};` to Zig's `const Foo = struct {};`: that shape is
    /// always an error (`c_style_container`), and anything else is `false`.
    fn parse_c_style_container(&mut self) -> R<bool> {
        match self.token_tag(self.tok_i) {
            Tag::KeywordEnum | Tag::KeywordUnion | Tag::KeywordStruct => {}
            _ => return Ok(false),
        }
        if self.token_tag(self.tok_i + 1) != Tag::Identifier {
            return Ok(false);
        }
        Err(ParseError) // c_style_container
    }

    fn parse_switch_prong_list(&mut self) -> R<()> {
        loop {
            if self.parse_switch_prong()?.is_none() {
                break;
            }
            match self.token_tag(self.tok_i) {
                Tag::Comma => self.tok_i += 1,
                // All possible delimiters.
                Tag::Colon | Tag::RParen | Tag::RBrace | Tag::RBracket => break,
                // Likely just a missing comma.
                _ => return Err(ParseError), // expected_comma_after_switch_prong
            }
        }
        Ok(())
    }

    fn parse_param_decl_list(&mut self) -> R<()> {
        self.expect_token(Tag::LParen)?;
        let mut varargs_seen = false;
        loop {
            if self.eat_token(Tag::RParen).is_some() {
                break;
            }
            if varargs_seen {
                return Err(ParseError); // varargs_nonfinal
            }
            let opt_param = self.expect_param_decl()?;
            if opt_param.is_none() && self.token_tag(self.tok_i - 1) == Tag::Ellipsis3 {
                varargs_seen = true;
            }
            match self.token_tag(self.tok_i) {
                Tag::Comma => self.tok_i += 1,
                Tag::RParen => {
                    self.tok_i += 1;
                    break;
                }
                Tag::Colon | Tag::RBrace | Tag::RBracket => return Err(ParseError), // expected r_paren
                // Likely just a missing comma.
                _ => return Err(ParseError), // expected_comma_after_param
            }
        }
        Ok(())
    }

    fn expect_builtin_call(&mut self) -> R<NodeIndex> {
        let builtin_token = self.assert_token(Tag::Builtin);
        if self.eat_token(Tag::LParen).is_none() {
            return Err(ParseError); // expected_param_list
        }
        loop {
            if self.eat_token(Tag::RParen).is_some() {
                break;
            }
            self.expect_expr()?;
            match self.token_tag(self.tok_i) {
                Tag::Comma => self.tok_i += 1,
                Tag::RParen => {
                    self.tok_i += 1;
                    break;
                }
                // Likely just a missing comma.
                _ => return Err(ParseError), // expected_comma_after_arg
            }
        }
        Ok(self.other(builtin_token, builtin_token))
    }

    fn body(&mut self, body: Body) -> R<NodeIndex> {
        match body {
            Body::Expr => self.expect_expr(),
            Body::TypeExpr => self.expect_type_expr(),
        }
    }

    fn expect_if(&mut self, body: Body) -> R<NodeIndex> {
        let if_token = self.assert_token(Tag::KeywordIf);
        self.expect_token(Tag::LParen)?;
        self.expect_expr()?;
        self.expect_token(Tag::RParen)?;
        self.parse_ptr_payload()?;

        self.body(body)?;

        if self.eat_token(Tag::KeywordElse).is_some() {
            self.parse_payload()?;
            self.body(body)?;
        }
        Ok(self.other(if_token, if_token))
    }

    fn expect_for(&mut self, body: Body, start: TokenIndex) -> R<NodeIndex> {
        let for_token = self.assert_token(Tag::KeywordFor);
        self.for_prefix()?;
        self.body(body)?;
        if self.eat_token(Tag::KeywordElse).is_some() {
            self.body(body)?;
        }
        Ok(self.other(for_token, start))
    }

    /// Skips over doc comment tokens. Returns the first one, if any.
    fn eat_doc_comments(&mut self) -> R<Option<TokenIndex>> {
        if let Some(tok) = self.eat_token(Tag::DocComment) {
            if tok > 0 && self.tokens_on_same_line(tok - 1, tok) {
                return Err(ParseError); // same_line_doc_comment
            }
            while self.eat_token(Tag::DocComment).is_some() {}
            return Ok(Some(tok));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepts(src: &str) -> bool {
        parse_zon(src.as_bytes()).is_ok()
    }

    #[test]
    fn zon_values_parse_into_their_nodes() {
        let ast = parse_zon(b".{ .a = 1, .b = .{ 2, 3 }, .c = -4, .d = (5) }").unwrap();
        let Kind::StructInit { type_expr: None, fields } = &ast.node(ast.root).kind else { panic!() };
        assert_eq!(4, fields.len());
        assert_eq!(Kind::NumberLiteral, ast.node(fields[0]).kind);
        assert!(matches!(ast.node(fields[1]).kind, Kind::ArrayInit { type_expr: None, .. }));
        assert!(matches!(ast.node(fields[2]).kind, Kind::Negation(_)));
        assert!(matches!(ast.node(fields[3]).kind, Kind::GroupedExpression(_)));
        // A field's name is two tokens before its value.
        assert_eq!(Tag::Identifier, ast.token_tag(ast.node(fields[1]).first_token - 2));
        assert_eq!(Kind::StructInit { type_expr: None, fields: vec![] }, parse_zon(b".{}").unwrap().nodes[0].kind);
    }

    #[test]
    fn any_zig_expression_parses_and_a_syntax_error_does_not() {
        for ok in [
            "1 + 2",
            "foo.bar(1, 2)",
            "struct { a: u8 = 1, const x = 2; fn f() void {} }",
            "if (a) b else c",
            "blk: { break :blk 1; }",
            "[_]u8{ 1, 2 }",
            "switch (x) { 1, 2 => a, else => b }",
            "@import(\"std\")",
            "error{ A, B }",
            "a orelse b catch |e| c",
            "[*c]u8",
            "*align(4:1:2) const u8",
            "fn (a: anytype, ...) callconv(.c) void",
        ] {
            assert!(accepts(ok), "{ok:?} parses");
        }
        for bad in [
            "",
            ".{ .a = }",
            "1 +2",
            "a == b == c",
            ".{ .a = 1 .b = 2 }",
            "struct Foo {}",
            "struct { a: u8, const x = 1; b: u8 }",
            ".{ /// doc\n .a = 1 }",
            "1 2",
            "a &&b",
            "[2]const u8",
            "//! top\n.{}",
        ] {
            assert!(!accepts(bad), "{bad:?} is refused");
        }
    }
}
