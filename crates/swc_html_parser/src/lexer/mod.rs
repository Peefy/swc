use std::{cell::RefCell, char::REPLACEMENT_CHARACTER, collections::VecDeque, mem::take, rc::Rc};

use swc_atoms::{Atom, JsWord};
use swc_common::{collections::AHashSet, input::Input, BytePos, Span};
use swc_html_ast::{AttributeToken, Raw, Token, TokenAndSpan};
use swc_html_utils::{Entity, HTML_ENTITIES};

use crate::{
    error::{Error, ErrorKind},
    parser::input::ParserInput,
};

#[derive(Debug, Clone)]
pub enum State {
    Data,
    Rcdata,
    Rawtext,
    ScriptData,
    PlainText,
    TagOpen,
    EndTagOpen,
    TagName,
    RcdataLessThanSign,
    RcdataEndTagOpen,
    RcdataEndTagName,
    RawtextLessThanSign,
    RawtextEndTagOpen,
    RawtextEndTagName,
    ScriptDataLessThanSign,
    ScriptDataEndTagOpen,
    ScriptDataEndTagName,
    ScriptDataEscapeStart,
    ScriptDataEscapeStartDash,
    ScriptDataEscaped,
    ScriptDataEscapedDash,
    ScriptDataEscapedDashDash,
    ScriptDataEscapedLessThanSign,
    ScriptDataEscapedEndTagOpen,
    ScriptDataEscapedEndTagName,
    ScriptDataDoubleEscapeStart,
    ScriptDataDoubleEscaped,
    ScriptDataDoubleEscapedDash,
    ScriptDataDoubleEscapedDashDash,
    ScriptDataDoubleEscapedLessThanSign,
    ScriptDataDoubleEscapeEnd,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    BogusComment,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentLessThanSign,
    CommentLessThanSignBang,
    CommentLessThanSignBangDash,
    CommentLessThanSignBangDashDash,
    CommentEndDash,
    CommentEnd,
    CommentEndBang,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
    AfterDoctypeName,
    AfterDoctypePublicKeyword,
    BeforeDoctypePublicIdentifier,
    DoctypePublicIdentifierDoubleQuoted,
    DoctypePublicIdentifierSingleQuoted,
    AfterDoctypePublicIdentifier,
    BetweenDoctypePublicAndSystemIdentifiers,
    AfterDoctypeSystemKeyword,
    BeforeDoctypeSystemIdentifier,
    DoctypeSystemIdentifierDoubleQuoted,
    DoctypeSystemIdentifierSingleQuoted,
    AfterDoctypeSystemIdentifier,
    BogusDoctype,
    CdataSection,
    CdataSectionBracket,
    CdataSectionEnd,
    CharacterReference,
    NamedCharacterReference,
    AmbiguousAmpersand,
    NumericCharacterReference,
    HexademicalCharacterReferenceStart,
    DecimalCharacterReferenceStart,
    HexademicalCharacterReference,
    DecimalCharacterReference,
    NumericCharacterReferenceEnd,
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct Doctype {
    name: Option<String>,
    force_quirks: bool,
    public_id: Option<String>,
    system_id: Option<String>,
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
enum TagKind {
    Start,
    End,
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct Tag {
    kind: TagKind,
    tag_name: String,
    raw_tag_name: Option<String>,
    is_self_closing: bool,
    attributes: Vec<Attribute>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct Attribute {
    span: Span,
    name: String,
    raw_name: Option<String>,
    value: Option<String>,
    raw_value: Option<String>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct Comment {
    data: String,
    raw: String,
}

pub(crate) type LexResult<T> = Result<T, ErrorKind>;

// TODO improve `raw` for all tokens (linting + better codegen)

pub struct Lexer<I>
where
    I: Input,
{
    input: I,
    cur: Option<char>,
    cur_pos: BytePos,
    last_token_pos: BytePos,
    finished: bool,
    state: State,
    return_state: State,
    errors: Vec<Error>,
    last_start_tag_name: Option<JsWord>,
    pending_tokens: VecDeque<TokenAndSpan>,
    current_doctype_token: Option<Doctype>,
    current_comment_token: Option<Comment>,
    doctype_raw: Option<String>,
    current_tag_token: Option<Tag>,
    attribute_start_position: Option<BytePos>,
    character_reference_code: Option<Vec<(u8, u32, Option<char>)>>,
    temporary_buffer: String,
    is_adjusted_current_node_is_element_in_html_namespace: Option<bool>,
    char_buf: Rc<RefCell<String>>,
}

impl<I> Lexer<I>
where
    I: Input,
{
    pub fn new(input: I) -> Self {
        let start_pos = input.last_pos();

        let mut lexer = Lexer {
            input,
            cur: None,
            cur_pos: start_pos,
            last_token_pos: start_pos,
            finished: false,
            state: State::Data,
            return_state: State::Data,
            errors: vec![],
            last_start_tag_name: None,
            pending_tokens: VecDeque::new(),
            current_doctype_token: None,
            doctype_raw: None,
            current_comment_token: None,
            current_tag_token: None,
            attribute_start_position: None,
            character_reference_code: None,
            // Do this without a new allocation.
            temporary_buffer: String::with_capacity(33),
            is_adjusted_current_node_is_element_in_html_namespace: None,
            char_buf: Rc::new(RefCell::new(String::with_capacity(2))),
        };

        // A leading Byte Order Mark (BOM) causes the character encoding argument to be
        // ignored and will itself be skipped.
        if lexer.input.is_at_start() && lexer.input.cur() == Some('\u{feff}') {
            lexer.input.bump();
        }

        lexer
    }
}

impl<I: Input> Iterator for Lexer<I> {
    type Item = TokenAndSpan;

    fn next(&mut self) -> Option<Self::Item> {
        let token_and_span = self.read_token_and_span();

        match token_and_span {
            Ok(token_and_span) => {
                return Some(token_and_span);
            }
            Err(..) => {
                return None;
            }
        }
    }
}

impl<I> ParserInput for Lexer<I>
where
    I: Input,
{
    fn start_pos(&mut self) -> swc_common::BytePos {
        self.input.cur_pos()
    }

    fn last_pos(&mut self) -> swc_common::BytePos {
        self.input.last_pos()
    }

    fn take_errors(&mut self) -> Vec<Error> {
        take(&mut self.errors)
    }

    fn set_last_start_tag_name(&mut self, tag_name: &JsWord) {
        self.last_start_tag_name = Some(tag_name.into());
    }

    fn set_adjusted_current_node_to_html_namespace(&mut self, value: bool) {
        self.is_adjusted_current_node_is_element_in_html_namespace = Some(value);
    }

    fn set_input_state(&mut self, state: State) {
        self.state = state;
    }
}

impl<I> Lexer<I>
where
    I: Input,
{
    #[inline(always)]
    fn next(&mut self) -> Option<char> {
        self.input.cur()
    }

    // Any occurrences of surrogates are surrogate-in-input-stream parse errors. Any
    // occurrences of noncharacters are noncharacter-in-input-stream parse errors
    // and any occurrences of controls other than ASCII whitespace and U+0000 NULL
    // characters are control-character-in-input-stream parse errors.
    //
    // Postpone validation for each character for perf reasons and do it in
    // `anything else`
    #[inline(always)]
    fn validate_input_stream_character(&mut self, c: char) {
        let code = c as u32;

        if (0xd800..=0xdfff).contains(&code) {
            self.emit_error(ErrorKind::SurrogateInInputStream);
        } else if code != 0x00 && is_control(code) {
            self.emit_error(ErrorKind::ControlCharacterInInputStream);
        } else if is_noncharacter(code) {
            self.emit_error(ErrorKind::NoncharacterInInputStream);
        }
    }

    #[inline(always)]
    fn consume(&mut self) {
        self.cur = self.input.cur();
        self.cur_pos = self.input.cur_pos();

        if self.cur.is_some() {
            self.input.bump();
        }
    }

    #[inline(always)]
    fn reconsume(&mut self) {
        self.input.reset_to(self.cur_pos);
    }

    #[inline(always)]
    fn reconsume_in_state(&mut self, state: State) {
        self.state = state;
        self.reconsume();
    }

    #[inline(always)]
    fn consume_next_char(&mut self) -> Option<char> {
        // The next input character is the first character in the input stream that has
        // not yet been consumed or explicitly ignored by the requirements in this
        // section. Initially, the next input character is the first character in the
        // input. The current input character is the last character to have been
        // consumed.
        let c = self.next();

        self.consume();

        c
    }

    #[cold]
    fn emit_error(&mut self, kind: ErrorKind) {
        self.errors.push(Error::new(
            Span::new(self.cur_pos, self.input.cur_pos(), Default::default()),
            kind,
        ));
    }

    #[inline(always)]
    fn emit_token(&mut self, token: Token) {
        let cur_pos = self.input.cur_pos();

        let span = Span::new(self.last_token_pos, cur_pos, Default::default());

        self.last_token_pos = cur_pos;
        self.pending_tokens.push_back(TokenAndSpan { span, token });
    }

    #[inline(always)]
    fn is_consumed_as_part_of_an_attribute(&mut self) -> bool {
        matches!(
            self.return_state,
            State::AttributeValueSingleQuoted
                | State::AttributeValueDoubleQuoted
                | State::AttributeValueUnquoted
        )
    }

    // An appropriate end tag token is an end tag token whose tag name matches the
    // tag name of the last start tag to have been emitted from this tokenizer, if
    // any. If no start tag has been emitted from this tokenizer, then no end tag
    // token is appropriate.
    #[inline(always)]
    fn current_end_tag_token_is_an_appropriate_end_tag_token(&mut self) -> bool {
        if let Some(last_start_tag_name) = &self.last_start_tag_name {
            if let Some(Tag {
                kind: TagKind::End,
                tag_name,
                ..
            }) = &self.current_tag_token
            {
                return last_start_tag_name == tag_name;
            }
        }

        false
    }

    #[inline(always)]
    fn emit_temporary_buffer_as_character_tokens(&mut self) {
        for c in take(&mut self.temporary_buffer).chars() {
            self.emit_token(Token::Character {
                value: c,
                raw: Some(Raw::Same),
            });
        }
    }

    fn flush_code_points_consumed_as_character_reference(&mut self, raw: Option<String>) {
        if self.is_consumed_as_part_of_an_attribute() {
            if let Some(Tag { attributes, .. }) = &mut self.current_tag_token {
                if let Some(attribute) = attributes.last_mut() {
                    // When the length of raw is more than the length of temporary buffer we emit a
                    // raw character in the first character token
                    let mut once_raw = raw;
                    let mut once_emitted = false;

                    for c in take(&mut self.temporary_buffer).chars() {
                        if let Some(old_value) = &mut attribute.value {
                            old_value.push(c);
                        } else {
                            let mut new_value = String::with_capacity(255);

                            new_value.push(c);

                            attribute.value = Some(new_value);
                        }

                        let raw = match once_raw {
                            Some(_) => {
                                once_emitted = true;
                                once_raw.take()
                            }
                            _ => {
                                if once_emitted {
                                    None
                                } else {
                                    Some(String::from(c))
                                }
                            }
                        };

                        if let Some(raw) = raw {
                            if let Some(raw_value) = &mut attribute.raw_value {
                                raw_value.push_str(&raw);
                            } else {
                                let mut raw_new_value = String::with_capacity(255);

                                raw_new_value.push_str(&raw);

                                attribute.raw_value = Some(raw_new_value);
                            }
                        }
                    }
                }
            }
        } else {
            // When the length of raw is more than the length of temporary buffer we emit a
            // raw character in the first character token
            let mut once_raw = raw;

            let is_value_eq_raw = if let Some(raw) = &once_raw {
                *raw == self.temporary_buffer
            } else {
                true
            };

            for c in take(&mut self.temporary_buffer).chars() {
                self.emit_token(Token::Character {
                    value: c,
                    raw: if is_value_eq_raw {
                        Some(Raw::Same)
                    } else {
                        once_raw.take().map(|x| Raw::Atom(Atom::new(x)))
                    },
                });
            }
        }
    }

    fn create_doctype_token(&mut self, name_c: Option<char>) {
        let mut new_name = None;

        if let Some(name_c) = name_c {
            let mut name = String::with_capacity(4);

            name.push(name_c);
            new_name = Some(name);
        }

        self.current_doctype_token = Some(Doctype {
            name: new_name,
            force_quirks: false,
            public_id: None,
            system_id: None,
        });
    }

    fn append_raw_to_doctype_token(&mut self, c: char) {
        if let Some(doctype_raw) = &mut self.doctype_raw {
            let is_cr = c == '\r';

            if is_cr {
                let mut raw = String::with_capacity(2);

                raw.push(c);

                if self.input.cur() == Some('\n') {
                    self.input.bump();

                    raw.push('\n');
                }

                doctype_raw.push_str(&raw);
            } else {
                doctype_raw.push(c);
            }
        }
    }

    fn append_to_doctype_token(
        &mut self,
        name: Option<char>,
        public_id: Option<char>,
        system_id: Option<char>,
    ) {
        if let Some(ref mut token) = self.current_doctype_token {
            if let Some(name) = name {
                if let Doctype {
                    name: Some(old_name),
                    ..
                } = token
                {
                    old_name.push(name);
                }
            }

            if let Some(public_id) = public_id {
                if let Doctype {
                    public_id: Some(old_public_id),
                    ..
                } = token
                {
                    old_public_id.push(public_id);
                }
            }

            if let Some(system_id) = system_id {
                if let Doctype {
                    system_id: Some(old_system_id),
                    ..
                } = token
                {
                    old_system_id.push(system_id);
                }
            }
        }
    }

    fn set_force_quirks(&mut self) {
        if let Some(Doctype { force_quirks, .. }) = &mut self.current_doctype_token {
            *force_quirks = true;
        }
    }

    fn set_doctype_token_public_id(&mut self) {
        if let Some(Doctype { public_id, .. }) = &mut self.current_doctype_token {
            // The Longest public id is `-//softquad software//dtd hotmetal pro
            // 6.0::19990601::extensions to html 4.0//`
            *public_id = Some(String::with_capacity(78));
        }
    }

    fn set_doctype_token_system_id(&mut self) {
        if let Some(Doctype { system_id, .. }) = &mut self.current_doctype_token {
            // The Longest system id is `http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd`
            *system_id = Some(String::with_capacity(58));
        }
    }

    fn emit_doctype_token(&mut self) {
        let current_doctype_token = self.current_doctype_token.take().unwrap();

        let raw = match self.doctype_raw.take() {
            Some(raw) => raw,
            _ => {
                unreachable!();
            }
        };

        let token = Token::Doctype {
            name: current_doctype_token.name.map(JsWord::from),
            force_quirks: current_doctype_token.force_quirks,
            public_id: current_doctype_token.public_id.map(JsWord::from),
            system_id: current_doctype_token.system_id.map(JsWord::from),
            raw: Some(Atom::new(raw)),
        };

        self.emit_token(token);
    }

    fn create_start_tag_token(&mut self) {
        self.current_tag_token = Some(Tag {
            kind: TagKind::Start,
            // Maximum known tag is `feComponentTransfer` (SVG)
            tag_name: String::with_capacity(19),
            raw_tag_name: Some(String::with_capacity(19)),
            is_self_closing: false,
            attributes: Vec::with_capacity(255),
        });
    }

    fn create_end_tag_token(&mut self) {
        self.current_tag_token = Some(Tag {
            kind: TagKind::End,
            // Maximum known tag is `feComponentTransfer` (SVG)
            tag_name: String::with_capacity(19),
            raw_tag_name: Some(String::with_capacity(19)),
            is_self_closing: false,
            attributes: Vec::with_capacity(255),
        });
    }

    fn append_to_tag_token_name(&mut self, c: char, raw_c: char) {
        if let Some(Tag {
            tag_name,
            raw_tag_name: Some(raw_tag_name),
            ..
        }) = &mut self.current_tag_token
        {
            tag_name.push(c);
            raw_tag_name.push(raw_c);
        }
    }

    fn start_new_attribute(&mut self) {
        if let Some(Tag { attributes, .. }) = &mut self.current_tag_token {
            // The longest known attribute is "glyph-orientation-horizontal" for SVG tags
            let name = String::with_capacity(28);
            let raw_name = String::with_capacity(28);

            attributes.push(Attribute {
                span: Default::default(),
                name,
                raw_name: Some(raw_name),
                value: None,
                raw_value: None,
            });

            self.attribute_start_position = Some(self.cur_pos);
        }
    }

    fn append_name_to_attribute(&mut self, c: char, raw_c: Option<char>) {
        if let Some(Tag { attributes, .. }) = &mut self.current_tag_token {
            if let Some(attribute) = attributes.last_mut() {
                attribute.name.push(c);

                if let Some(raw_c) = raw_c {
                    if let Some(raw_name) = &mut attribute.raw_name {
                        raw_name.push(raw_c);
                    }
                }
            }
        }
    }

    fn append_value_to_attribute(&mut self, quotes: bool, c: Option<char>, raw_c: Option<char>) {
        if let Some(Tag { attributes, .. }) = &mut self.current_tag_token {
            if let Some(attribute) = attributes.last_mut() {
                if let Some(c) = c {
                    if let Some(old_value) = &mut attribute.value {
                        old_value.push(c);
                    } else {
                        let mut new_value = String::with_capacity(255);

                        new_value.push(c);

                        attribute.value = Some(new_value);
                    }
                }

                if let Some(raw_c) = raw_c {
                    // Quote for attribute was found, so we set empty value by default
                    if quotes && attribute.value.is_none() {
                        attribute.value = Some(String::with_capacity(255));
                    }

                    if let Some(raw_value) = &mut attribute.raw_value {
                        raw_value.push(raw_c);
                    } else {
                        let mut raw_new_value = String::with_capacity(255);

                        raw_new_value.push(raw_c);

                        attribute.raw_value = Some(raw_new_value);
                    }
                }
            }
        }
    }

    fn update_attribute_span(&mut self) {
        if let Some(attribute_start_position) = self.attribute_start_position {
            if let Some(Tag {
                ref mut attributes, ..
            }) = self.current_tag_token
            {
                if let Some(last) = attributes.last_mut() {
                    last.span =
                        Span::new(attribute_start_position, self.cur_pos, Default::default());
                }
            }
        }
    }

    fn emit_tag_token(&mut self) {
        if let Some(mut current_tag_token) = self.current_tag_token.take() {
            match current_tag_token.kind {
                TagKind::Start => {
                    self.last_start_tag_name = Some(current_tag_token.tag_name.clone().into());

                    let mut already_seen: AHashSet<JsWord> = Default::default();

                    let start_tag_token = Token::StartTag {
                        tag_name: current_tag_token.tag_name.into(),
                        raw_tag_name: current_tag_token.raw_tag_name.map(Atom::new),
                        is_self_closing: current_tag_token.is_self_closing,
                        attributes: current_tag_token
                            .attributes
                            .drain(..)
                            .map(|attribute| {
                                let name: JsWord = JsWord::from(attribute.name);

                                if already_seen.contains(&name) {
                                    self.errors.push(Error::new(
                                        attribute.span,
                                        ErrorKind::DuplicateAttribute,
                                    ));
                                }

                                already_seen.insert(name.clone());

                                AttributeToken {
                                    span: attribute.span,
                                    name,
                                    raw_name: attribute.raw_name.map(Atom::new),
                                    value: attribute.value.map(JsWord::from),
                                    raw_value: attribute.raw_value.map(Atom::new),
                                }
                            })
                            .collect(),
                    };

                    self.emit_token(start_tag_token);
                }
                TagKind::End => {
                    if !current_tag_token.attributes.is_empty() {
                        self.emit_error(ErrorKind::EndTagWithAttributes);
                    }

                    if current_tag_token.is_self_closing {
                        self.emit_error(ErrorKind::EndTagWithTrailingSolidus);
                    }

                    let mut already_seen: AHashSet<JsWord> = Default::default();

                    let end_tag_token = Token::EndTag {
                        tag_name: current_tag_token.tag_name.into(),
                        raw_tag_name: current_tag_token.raw_tag_name.map(Atom::new),
                        is_self_closing: current_tag_token.is_self_closing,
                        attributes: current_tag_token
                            .attributes
                            .drain(..)
                            .map(|attribute| {
                                let name: JsWord = JsWord::from(attribute.name);

                                if already_seen.contains(&name) {
                                    self.errors.push(Error::new(
                                        attribute.span,
                                        ErrorKind::DuplicateAttribute,
                                    ));
                                }

                                already_seen.insert(name.clone());

                                AttributeToken {
                                    span: attribute.span,
                                    name,
                                    raw_name: attribute.raw_name.map(Atom::new),
                                    value: attribute.value.map(JsWord::from),
                                    raw_value: attribute.raw_value.map(Atom::new),
                                }
                            })
                            .collect(),
                    };

                    self.emit_token(end_tag_token);
                }
            }
        }
    }

    fn create_comment_token(&mut self, new_data: Option<String>, raw_start: &str) {
        let mut data = String::with_capacity(64);
        let mut raw = String::with_capacity(71);

        raw.push_str(raw_start);

        if let Some(new_data) = new_data {
            data.push_str(&new_data);
            raw.push_str(&new_data);
        };

        self.current_comment_token = Some(Comment { data, raw });
    }

    fn append_to_comment_token(&mut self, c: char, raw_c: char) {
        if let Some(Comment { data, raw }) = &mut self.current_comment_token {
            data.push(c);
            raw.push(raw_c);
        }
    }

    fn handle_raw_and_append_to_comment_token(&mut self, c: char) {
        if let Some(Comment { data, raw }) = &mut self.current_comment_token {
            let is_cr = c == '\r';

            if is_cr {
                let mut raw_c = String::with_capacity(2);

                raw_c.push(c);

                if self.input.cur() == Some('\n') {
                    self.input.bump();

                    raw_c.push('\n');
                }

                data.push('\n');
                raw.push_str(&raw_c);
            } else {
                data.push(c);
                raw.push(c);
            }
        }
    }

    fn emit_comment_token(&mut self, raw_end: Option<&str>) {
        let mut comment = self.current_comment_token.take().unwrap();

        if let Some(raw_end) = raw_end {
            comment.raw.push_str(raw_end);
        }

        self.emit_token(Token::Comment {
            data: comment.data.into(),
            raw: Some(Atom::new(comment.raw)),
        });
    }

    fn with_char_buf<F, Ret>(&mut self, op: F) -> LexResult<Ret>
    where
        F: for<'any> FnOnce(&mut Lexer<I>, &mut String) -> LexResult<Ret>,
    {
        let b = self.char_buf.clone();
        let mut buf = b.borrow_mut();

        buf.clear();

        op(self, &mut buf)
    }

    #[inline(always)]
    fn emit_character_token(&mut self, value: char) -> LexResult<()> {
        self.emit_token(Token::Character {
            value,
            raw: Some(Raw::Same),
        });

        Ok(())
    }

    #[inline(always)]
    fn emit_character_token_with_raw(&mut self, value: (char, char)) -> LexResult<()> {
        self.with_char_buf(|l, buf| {
            buf.push(value.1);

            l.emit_token(Token::Character {
                value: value.0,
                raw: Some(Raw::Atom(Atom::new(&**buf))),
            });

            Ok(())
        })
    }

    fn handle_raw_and_emit_character_token(&mut self, c: char) -> LexResult<()> {
        let is_cr = c == '\r';

        if is_cr {
            self.with_char_buf(|l, buf| {
                buf.push(c);

                if l.input.cur() == Some('\n') {
                    l.input.bump();

                    buf.push('\n');
                }

                l.emit_token(Token::Character {
                    value: '\n',
                    raw: Some(Raw::Atom(Atom::new(&**buf))),
                });

                Ok(())
            })
        } else {
            self.emit_token(Token::Character {
                value: c,
                raw: Some(Raw::Same),
            });

            Ok(())
        }
    }

    fn read_token_and_span(&mut self) -> LexResult<TokenAndSpan> {
        if self.finished {
            return Err(ErrorKind::Eof);
        } else {
            while self.pending_tokens.is_empty() {
                self.run()?;
            }
        }

        let token_and_span = self.pending_tokens.pop_front().unwrap();

        match token_and_span.token {
            Token::Eof => {
                self.finished = true;

                return Err(ErrorKind::Eof);
            }
            _ => {
                return Ok(token_and_span);
            }
        }
    }

    fn run(&mut self) -> LexResult<()> {
        match self.state {
            // https://html.spec.whatwg.org/multipage/parsing.html#data-state
            State::Data => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0026 AMPERSAND (&)
                    // Set the return state to the data state. Switch to the character reference
                    // state.
                    Some('&') => {
                        self.return_state = State::Data;
                        self.state = State::CharacterReference;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the tag open state.
                    Some('<') => {
                        self.state = State::TagOpen;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Emit the current input
                    // character as a character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.emit_character_token(c)?;
                    }
                    // EOF
                    // Emit an end-of-file token.
                    None => {
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Emit the current input character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#rcdata-state
            State::Rcdata => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0026 AMPERSAND (&)
                    // Set the return state to the RCDATA state. Switch to the character
                    // reference state.
                    Some('&') => {
                        self.return_state = State::Rcdata;
                        self.state = State::CharacterReference;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the RCDATA less-than sign state.
                    Some('<') => {
                        self.state = State::RcdataLessThanSign;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Emit a U+FFFD
                    // REPLACEMENT CHARACTER character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // Emit an end-of-file token.
                    None => {
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Emit the current input character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#rawtext-state
            State::Rawtext => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the RAWTEXT less-than sign state.
                    Some('<') => self.state = State::RawtextLessThanSign,
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Emit a U+FFFD
                    // REPLACEMENT CHARACTER character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // Emit an end-of-file token.
                    None => {
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Emit the current input character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-state
            State::ScriptData => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the script data less-than sign state.
                    Some('<') => self.state = State::ScriptDataLessThanSign,
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Emit a U+FFFD
                    // REPLACEMENT CHARACTER character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // Emit an end-of-file token.
                    None => {
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Emit the current input character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#plaintext-state
            State::PlainText => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Emit a U+FFFD
                    // REPLACEMENT CHARACTER character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // Emit an end-of-file token.
                    None => {
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Emit the current input character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#tag-open-state
            State::TagOpen => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0021 EXCLAMATION MARK (!)
                    // Switch to the markup declaration open state.
                    Some('!') => {
                        self.state = State::MarkupDeclarationOpen;
                    }
                    // U+002F SOLIDUS (/)
                    // Switch to the end tag open state.
                    Some('/') => {
                        self.state = State::EndTagOpen;
                    }
                    // ASCII alpha
                    // Create a new start tag token, set its tag name to the empty string.
                    // Reconsume in the tag name state.
                    Some(c) if is_ascii_alpha(c) => {
                        self.create_start_tag_token();
                        self.reconsume_in_state(State::TagName);
                    }
                    // U+003F QUESTION MARK (?)
                    // This is an unexpected-question-mark-instead-of-tag-name parse error.
                    // Create a comment token whose data is the empty string. Reconsume in the
                    // bogus comment state.
                    Some('?') => {
                        self.emit_error(ErrorKind::UnexpectedQuestionMarkInsteadOfTagName);
                        self.create_comment_token(None, "<");
                        self.reconsume_in_state(State::BogusComment);
                    }
                    // EOF
                    // This is an eof-before-tag-name parse error. Emit a U+003C LESS-THAN SIGN
                    // character token and an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofBeforeTagName);
                        self.emit_character_token('<')?;
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is an invalid-first-character-of-tag-name parse error. Emit a U+003C
                    // LESS-THAN SIGN character token. Reconsume in the data state.
                    _ => {
                        self.emit_error(ErrorKind::InvalidFirstCharacterOfTagName);
                        self.emit_character_token('<')?;
                        self.reconsume_in_state(State::Data);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#end-tag-open-state
            State::EndTagOpen => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII alpha
                    // Create a new end tag token, set its tag name to the empty string.
                    // Reconsume in the tag name state.
                    Some(c) if is_ascii_alpha(c) => {
                        self.create_end_tag_token();
                        self.reconsume_in_state(State::TagName);
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is a missing-end-tag-name parse error. Switch to the data state.
                    Some('>') => {
                        self.emit_error(ErrorKind::MissingEndTagName);
                        self.state = State::Data;
                    }
                    // EOF
                    // This is an eof-before-tag-name parse error. Emit a U+003C LESS-THAN SIGN
                    // character token, a U+002F SOLIDUS character token and an end-of-file
                    // token.
                    None => {
                        self.emit_error(ErrorKind::EofBeforeTagName);
                        self.emit_character_token('<')?;
                        self.emit_character_token('/')?;
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is an invalid-first-character-of-tag-name parse error. Create a
                    // comment token whose data is the empty string. Reconsume in the bogus
                    // comment state.
                    _ => {
                        self.emit_error(ErrorKind::InvalidFirstCharacterOfTagName);
                        self.create_comment_token(None, "</");
                        self.reconsume_in_state(State::BogusComment);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#tag-name-state
            State::TagName => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Switch to the before attribute name state.
                    Some(c) if is_spacy(c) => {
                        self.skip_next_lf(c);
                        self.state = State::BeforeAttributeName;
                    }
                    // U+002F SOLIDUS (/)
                    // Switch to the self-closing start tag state.
                    Some('/') => {
                        self.state = State::SelfClosingStartTag;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current tag token.
                    Some('>') => {
                        self.state = State::Data;
                        self.emit_tag_token();
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the current tag token's tag name.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.append_to_tag_token_name(c.to_ascii_lowercase(), c);
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current tag token's tag name.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_to_tag_token_name(REPLACEMENT_CHARACTER, c);
                    }
                    // EOF
                    // This is an eof-in-tag parse error. Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInTag);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current tag token's tag name.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_to_tag_token_name(c, c);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#rcdata-less-than-sign-state
            State::RcdataLessThanSign => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002F SOLIDUS (/)
                    // Set the temporary buffer to the empty string. Switch to the RCDATA end
                    // tag open state.
                    Some('/') => {
                        self.temporary_buffer.clear();
                        self.state = State::RcdataEndTagOpen;
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token. Reconsume in the RCDATA
                    // state.
                    _ => {
                        self.emit_character_token('<')?;
                        self.reconsume_in_state(State::Rcdata);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#rcdata-end-tag-open-state
            State::RcdataEndTagOpen => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII alpha
                    // Create a new end tag token, set its tag name to the empty string.
                    // Reconsume in the RCDATA end tag name state.
                    Some(c) if is_ascii_alpha(c) => {
                        self.create_end_tag_token();
                        self.reconsume_in_state(State::RcdataEndTagName);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token and a U+002F SOLIDUS
                    // character token. Reconsume in the RCDATA state.
                    _ => {
                        self.emit_character_token('<')?;
                        self.emit_character_token('/')?;
                        self.reconsume_in_state(State::Rcdata);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#rcdata-end-tag-name-state
            State::RcdataEndTagName => {
                let anything_else = |lexer: &mut Lexer<I>| -> LexResult<()> {
                    lexer.emit_character_token('<')?;
                    lexer.emit_character_token('/')?;
                    lexer.emit_temporary_buffer_as_character_tokens();
                    lexer.reconsume_in_state(State::Rcdata);

                    Ok(())
                };

                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the before attribute name state. Otherwise, treat it as per the
                    // "anything else" entry below.
                    Some(c) if is_spacy(c) => {
                        self.skip_next_lf(c);

                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::BeforeAttributeName;
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // U+002F SOLIDUS (/)
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the self-closing start tag state. Otherwise, treat it as per the
                    // "anything else" entry below.
                    Some('/') => {
                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::SelfClosingStartTag;
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the data state and emit the current tag token. Otherwise, treat it as
                    // per the "anything else" entry below.
                    Some('>') => {
                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::Data;
                            self.emit_tag_token();
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the current tag token's tag name.
                    // Append the current input character to the temporary buffer.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.append_to_tag_token_name(c.to_ascii_lowercase(), c);
                        self.temporary_buffer.push(c);
                    }
                    // ASCII lower alpha
                    // Append the current input character to the current tag token's tag name.
                    // Append the current input character to the temporary buffer.
                    Some(c) if is_ascii_lower_alpha(c) => {
                        self.append_to_tag_token_name(c, c);
                        self.temporary_buffer.push(c);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token, a U+002F SOLIDUS character
                    // token, and a character token for each of the characters in the temporary
                    // buffer (in the order they were added to the buffer). Reconsume in the
                    // RCDATA state.
                    _ => {
                        anything_else(self)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#rawtext-less-than-sign-state
            State::RawtextLessThanSign => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002F SOLIDUS (/)
                    // Set the temporary buffer to the empty string. Switch to the RAWTEXT end
                    // tag open state.
                    Some('/') => {
                        self.temporary_buffer.clear();
                        self.state = State::RawtextEndTagOpen;
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token. Reconsume in the RAWTEXT
                    // state.
                    _ => {
                        self.emit_character_token('<')?;
                        self.reconsume_in_state(State::Rawtext);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#rawtext-end-tag-open-state
            State::RawtextEndTagOpen => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII alpha
                    // Create a new end tag token, set its tag name to the empty string.
                    // Reconsume in the RAWTEXT end tag name state.
                    Some(c) if is_ascii_alpha(c) => {
                        self.create_end_tag_token();
                        self.reconsume_in_state(State::RawtextEndTagName);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token and a U+002F SOLIDUS
                    // character token. Reconsume in the RAWTEXT state.
                    _ => {
                        self.emit_character_token('<')?;
                        self.emit_character_token('/')?;
                        self.reconsume_in_state(State::Rawtext);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#rawtext-end-tag-name-state
            State::RawtextEndTagName => {
                let anything_else = |lexer: &mut Lexer<I>| -> LexResult<()> {
                    lexer.emit_character_token('<')?;
                    lexer.emit_character_token('/')?;
                    lexer.emit_temporary_buffer_as_character_tokens();
                    lexer.reconsume_in_state(State::Rawtext);

                    Ok(())
                };

                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the before attribute name state. Otherwise, treat it as per the
                    // "anything else" entry below.
                    Some(c) if is_spacy(c) => {
                        self.skip_next_lf(c);

                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::BeforeAttributeName;
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // U+002F SOLIDUS (/)
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the self-closing start tag state. Otherwise, treat it as per the
                    // "anything else" entry below.
                    Some('/') => {
                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::SelfClosingStartTag;
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the data state and emit the current tag token. Otherwise, treat it as
                    // per the "anything else" entry below.
                    Some('>') => {
                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::Data;
                            self.emit_tag_token();
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the current tag token's tag name.
                    // Append the current input character to the temporary buffer.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.append_to_tag_token_name(c.to_ascii_lowercase(), c);
                        self.temporary_buffer.push(c);
                    }
                    // ASCII lower alpha
                    // Append the current input character to the current tag token's tag name.
                    // Append the current input character to the temporary buffer.
                    Some(c) if is_ascii_lower_alpha(c) => {
                        self.append_to_tag_token_name(c, c);
                        self.temporary_buffer.push(c);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token, a U+002F SOLIDUS character
                    // token, and a character token for each of the characters in the temporary
                    // buffer (in the order they were added to the buffer). Reconsume in the
                    // RAWTEXT state.
                    _ => {
                        anything_else(self)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-less-than-sign-state
            State::ScriptDataLessThanSign => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002F SOLIDUS (/)
                    // Set the temporary buffer to the empty string. Switch to the script data
                    // end tag open state.
                    Some('/') => {
                        self.temporary_buffer.clear();
                        self.state = State::ScriptDataEndTagOpen;
                    }
                    // U+0021 EXCLAMATION MARK (!)
                    // Switch to the script data escape start state. Emit a U+003C LESS-THAN
                    // SIGN character token and a U+0021 EXCLAMATION MARK character token.
                    Some('!') => {
                        self.state = State::ScriptDataEscapeStart;
                        self.emit_character_token('<')?;
                        self.emit_character_token('!')?;
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token. Reconsume in the script
                    // data state.
                    _ => {
                        self.emit_character_token('<')?;
                        self.reconsume_in_state(State::ScriptData);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-end-tag-open-state
            State::ScriptDataEndTagOpen => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII alpha
                    // Create a new end tag token, set its tag name to the empty string.
                    // Reconsume in the script data end tag name state.
                    Some(c) if is_ascii_alpha(c) => {
                        self.create_end_tag_token();
                        self.reconsume_in_state(State::ScriptDataEndTagName);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token and a U+002F SOLIDUS
                    // character token. Reconsume in the script data state.
                    _ => {
                        self.emit_character_token('<')?;
                        self.emit_character_token('/')?;
                        self.reconsume_in_state(State::ScriptData);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-end-tag-name-state
            State::ScriptDataEndTagName => {
                let anything_else = |lexer: &mut Lexer<I>| -> LexResult<()> {
                    lexer.emit_character_token('<')?;
                    lexer.emit_character_token('/')?;
                    lexer.emit_temporary_buffer_as_character_tokens();
                    lexer.reconsume_in_state(State::ScriptData);

                    Ok(())
                };

                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the before attribute name state. Otherwise, treat it as per the
                    // "anything else" entry below.
                    Some(c) if is_spacy(c) => {
                        self.skip_next_lf(c);

                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::BeforeAttributeName;
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // U+002F SOLIDUS (/)
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the self-closing start tag state. Otherwise, treat it as per the
                    // "anything else" entry below.
                    Some('/') => {
                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::SelfClosingStartTag;
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the data state and emit the current tag token. Otherwise, treat it as
                    // per the "anything else" entry below.
                    Some('>') => {
                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::Data;
                            self.emit_tag_token();
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the current tag token's tag name.
                    // Append the current input character to the temporary buffer.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.append_to_tag_token_name(c.to_ascii_lowercase(), c);
                        self.temporary_buffer.push(c);
                    }
                    // ASCII lower alpha
                    // Append the current input character to the current tag token's tag name.
                    // Append the current input character to the temporary buffer.
                    Some(c) if is_ascii_lower_alpha(c) => {
                        self.append_to_tag_token_name(c, c);
                        self.temporary_buffer.push(c);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token, a U+002F SOLIDUS character
                    // token, and a character token for each of the characters in the temporary
                    // buffer (in the order they were added to the buffer). Reconsume in the
                    // script data state.
                    _ => {
                        anything_else(self)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-escape-start-state
            State::ScriptDataEscapeStart => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the script data escape start dash state. Emit a U+002D
                    // HYPHEN-MINUS character token.
                    Some(c @ '-') => {
                        self.state = State::ScriptDataEscapeStartDash;
                        self.emit_character_token(c)?;
                    }
                    // Anything else
                    // Reconsume in the script data state.
                    _ => {
                        self.reconsume_in_state(State::ScriptData);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-escape-start-dash-state
            State::ScriptDataEscapeStartDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the script data escaped dash dash state. Emit a U+002D
                    // HYPHEN-MINUS character token.
                    Some(c @ '-') => {
                        self.state = State::ScriptDataEscapedDashDash;
                        self.emit_character_token(c)?;
                    }
                    // Anything else
                    // Reconsume in the script data state.
                    _ => {
                        self.reconsume_in_state(State::ScriptData);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-escaped-state
            State::ScriptDataEscaped => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the script data escaped dash state. Emit a U+002D HYPHEN-MINUS
                    // character token.
                    Some(c @ '-') => {
                        self.state = State::ScriptDataEscapedDash;
                        self.emit_character_token(c)?;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the script data escaped less-than sign state.
                    Some('<') => {
                        self.state = State::ScriptDataEscapedLessThanSign;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Emit a U+FFFD
                    // REPLACEMENT CHARACTER character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // This is an eof-in-script-html-comment-like-text parse error. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInScriptHtmlCommentLikeText);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Emit the current input character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-escaped-dash-state
            State::ScriptDataEscapedDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the script data escaped dash dash state. Emit a U+002D
                    // HYPHEN-MINUS character token.
                    Some(c @ '-') => {
                        self.state = State::ScriptDataEscapedDashDash;
                        self.emit_character_token(c)?;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the script data escaped less-than sign state.
                    Some('<') => {
                        self.state = State::ScriptDataEscapedLessThanSign;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Switch to the script
                    // data escaped state. Emit a U+FFFD REPLACEMENT CHARACTER character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.state = State::ScriptDataEscaped;
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // This is an eof-in-script-html-comment-like-text parse error. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInScriptHtmlCommentLikeText);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Switch to the script data escaped state. Emit the current input character
                    // as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.state = State::ScriptDataEscaped;
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-escaped-dash-dash-state
            State::ScriptDataEscapedDashDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Emit a U+002D HYPHEN-MINUS character token.
                    Some(c @ '-') => {
                        self.emit_character_token(c)?;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the script data escaped less-than sign state.
                    Some('<') => {
                        self.state = State::ScriptDataEscapedLessThanSign;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the script data state. Emit a U+003E GREATER-THAN SIGN
                    // character token.
                    Some(c @ '>') => {
                        self.state = State::ScriptData;
                        self.emit_character_token(c)?;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Switch to the script
                    // data escaped state. Emit a U+FFFD REPLACEMENT CHARACTER character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.state = State::ScriptDataEscaped;
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // This is an eof-in-script-html-comment-like-text parse error. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInScriptHtmlCommentLikeText);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Switch to the script data escaped state. Emit the current input character
                    // as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.state = State::ScriptDataEscaped;
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-escaped-less-than-sign-state
            State::ScriptDataEscapedLessThanSign => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002F SOLIDUS (/)
                    // Set the temporary buffer to the empty string. Switch to the script data
                    // escaped end tag open state.
                    Some('/') => {
                        self.temporary_buffer.clear();
                        self.state = State::ScriptDataEscapedEndTagOpen;
                    }
                    // ASCII alpha
                    // Set the temporary buffer to the empty string. Emit a U+003C LESS-THAN
                    // SIGN character token. Reconsume in the script data double escape start
                    // state.
                    Some(c) if is_ascii_alpha(c) => {
                        self.temporary_buffer.clear();
                        self.emit_character_token('<')?;
                        self.reconsume_in_state(State::ScriptDataDoubleEscapeStart);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token. Reconsume in the script
                    // data escaped state.
                    _ => {
                        self.emit_character_token('<')?;
                        self.reconsume_in_state(State::ScriptDataEscaped);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-escaped-end-tag-open-state
            State::ScriptDataEscapedEndTagOpen => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII alpha
                    // Create a new end tag token, set its tag name to the empty string.
                    // Reconsume in the script data escaped end tag name state.
                    Some(c) if is_ascii_alpha(c) => {
                        self.create_end_tag_token();
                        self.reconsume_in_state(State::ScriptDataEscapedEndTagName);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token and a U+002F SOLIDUS
                    // character token. Reconsume in the script data escaped state.
                    _ => {
                        self.emit_character_token('<')?;
                        self.emit_character_token('/')?;
                        self.reconsume_in_state(State::ScriptDataEscaped);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-escaped-end-tag-name-state
            State::ScriptDataEscapedEndTagName => {
                let anything_else = |lexer: &mut Lexer<I>| -> LexResult<()> {
                    lexer.emit_character_token('<')?;
                    lexer.emit_character_token('/')?;
                    lexer.emit_temporary_buffer_as_character_tokens();
                    lexer.reconsume_in_state(State::ScriptDataEscaped);

                    Ok(())
                };

                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the before attribute name state. Otherwise, treat it as per the
                    // "anything else" entry below.
                    Some(c) if is_spacy(c) => {
                        self.skip_next_lf(c);

                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::BeforeAttributeName;
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // U+002F SOLIDUS (/)
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the self-closing start tag state. Otherwise, treat it as per the
                    // "anything else" entry below.
                    Some('/') => {
                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::SelfClosingStartTag;
                        } else {
                            anything_else(self)?;
                        }
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // If the current end tag token is an appropriate end tag token, then switch
                    // to the data state and emit the current tag token. Otherwise, treat it as
                    // per the "anything else" entry below.
                    Some('>') => {
                        if self.current_end_tag_token_is_an_appropriate_end_tag_token() {
                            self.state = State::Data;
                            self.emit_tag_token();
                        } else {
                            anything_else(self)?
                        }
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the current tag token's tag name.
                    // Append the current input character to the temporary buffer.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.append_to_tag_token_name(c.to_ascii_lowercase(), c);
                        self.temporary_buffer.push(c);
                    }
                    // ASCII lower alpha
                    // Append the current input character to the current tag token's tag name.
                    // Append the current input character to the temporary buffer.
                    Some(c) if is_ascii_lower_alpha(c) => {
                        self.append_to_tag_token_name(c, c);

                        self.temporary_buffer.push(c);
                    }
                    // Anything else
                    // Emit a U+003C LESS-THAN SIGN character token, a U+002F SOLIDUS character
                    // token, and a character token for each of the characters in the temporary
                    // buffer (in the order they were added to the buffer). Reconsume in the
                    // script data escaped state.
                    _ => {
                        anything_else(self)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-double-escape-start-state
            State::ScriptDataDoubleEscapeStart => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // U+002F SOLIDUS (/)
                    // U+003E GREATER-THAN SIGN (>)
                    // If the temporary buffer is the string "script", then switch to the script
                    // data double escaped state. Otherwise, switch to the script data escaped
                    // state. Emit the current input character as a character token.
                    Some(c) if is_spacy(c) => {
                        let is_script = self.temporary_buffer == "script";

                        if is_script {
                            self.state = State::ScriptDataDoubleEscaped;
                        } else {
                            self.state = State::ScriptDataEscaped;
                        }

                        self.handle_raw_and_emit_character_token(c)?;
                    }
                    Some(c @ '/' | c @ '>') => {
                        let is_script = self.temporary_buffer == "script";

                        if is_script {
                            self.state = State::ScriptDataDoubleEscaped;
                        } else {
                            self.state = State::ScriptDataEscaped;
                        }

                        self.emit_character_token(c)?;
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the temporary buffer. Emit the current
                    // input character as a character token.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.temporary_buffer.push(c.to_ascii_lowercase());
                        self.emit_character_token(c)?;
                    }
                    // ASCII lower alpha
                    // Append the current input character to the temporary buffer. Emit the
                    // current input character as a character token.
                    Some(c) if is_ascii_lower_alpha(c) => {
                        self.temporary_buffer.push(c);
                        self.emit_character_token(c)?;
                    }
                    // Anything else
                    // Reconsume in the script data escaped state.
                    _ => {
                        self.reconsume_in_state(State::ScriptDataEscaped);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-double-escaped-state
            State::ScriptDataDoubleEscaped => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the script data double escaped dash state. Emit a U+002D
                    // HYPHEN-MINUS character token.
                    Some(c @ '-') => {
                        self.state = State::ScriptDataDoubleEscapedDash;
                        self.emit_character_token(c)?;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the script data double escaped less-than sign state. Emit a
                    // U+003C LESS-THAN SIGN character token.
                    Some(c @ '<') => {
                        self.state = State::ScriptDataDoubleEscapedLessThanSign;
                        self.emit_character_token(c)?;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Emit a U+FFFD
                    // REPLACEMENT CHARACTER character token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // This is an eof-in-script-html-comment-like-text parse error. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInScriptHtmlCommentLikeText);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Emit the current input character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-double-escaped-dash-state
            State::ScriptDataDoubleEscapedDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the script data double escaped dash dash state. Emit a U+002D
                    // HYPHEN-MINUS character token.
                    Some(c @ '-') => {
                        self.state = State::ScriptDataDoubleEscapedDashDash;
                        self.emit_character_token(c)?;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the script data double escaped less-than sign state. Emit a
                    // U+003C LESS-THAN SIGN character token.
                    Some(c @ '<') => {
                        self.state = State::ScriptDataDoubleEscapedLessThanSign;
                        self.emit_character_token(c)?;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Switch to the script
                    // data double escaped state. Emit a U+FFFD REPLACEMENT CHARACTER character
                    // token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.state = State::ScriptDataDoubleEscaped;
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // This is an eof-in-script-html-comment-like-text parse error. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInScriptHtmlCommentLikeText);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Switch to the script data double escaped state. Emit the current input
                    // character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.state = State::ScriptDataDoubleEscaped;
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-double-escaped-dash-dash-state
            State::ScriptDataDoubleEscapedDashDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Emit a U+002D HYPHEN-MINUS character token.
                    Some(c @ '-') => {
                        self.emit_character_token(c)?;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Switch to the script data double escaped less-than sign state. Emit a
                    // U+003C LESS-THAN SIGN character token.
                    Some(c @ '<') => {
                        self.state = State::ScriptDataDoubleEscapedLessThanSign;
                        self.emit_character_token(c)?;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the script data state. Emit a U+003E GREATER-THAN SIGN
                    // character token.
                    Some(c @ '>') => {
                        self.state = State::ScriptData;
                        self.emit_character_token(c)?;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Switch to the script
                    // data double escaped state. Emit a U+FFFD REPLACEMENT CHARACTER character
                    // token.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.state = State::ScriptDataDoubleEscaped;
                        self.emit_character_token_with_raw((REPLACEMENT_CHARACTER, c))?;
                    }
                    // EOF
                    // This is an eof-in-script-html-comment-like-text parse error. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInScriptHtmlCommentLikeText);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Switch to the script data double escaped state. Emit the current input
                    // character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.state = State::ScriptDataDoubleEscaped;
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-double-escaped-less-than-sign-state
            State::ScriptDataDoubleEscapedLessThanSign => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002F SOLIDUS (/)
                    // Set the temporary buffer to the empty string. Switch to the script data
                    // double escape end state. Emit a U+002F SOLIDUS character token.
                    Some(c @ '/') => {
                        self.temporary_buffer.clear();
                        self.state = State::ScriptDataDoubleEscapeEnd;
                        self.emit_character_token(c)?;
                    }
                    // Anything else
                    // Reconsume in the script data double escaped state.
                    _ => {
                        self.reconsume_in_state(State::ScriptDataDoubleEscaped);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#script-data-double-escape-end-state
            State::ScriptDataDoubleEscapeEnd => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // U+002F SOLIDUS (/)
                    // U+003E GREATER-THAN SIGN (>)
                    // If the temporary buffer is the string "script", then switch to the script
                    // data escaped state. Otherwise, switch to the script data double escaped
                    // state. Emit the current input character as a character token.
                    Some(c) if is_spacy(c) => {
                        let is_script = self.temporary_buffer == "script";

                        if is_script {
                            self.state = State::ScriptDataEscaped;
                        } else {
                            self.state = State::ScriptDataDoubleEscaped;
                        }

                        self.handle_raw_and_emit_character_token(c)?;
                    }
                    Some(c @ '/' | c @ '>') => {
                        let is_script = self.temporary_buffer == "script";

                        if is_script {
                            self.state = State::ScriptDataEscaped;
                        } else {
                            self.state = State::ScriptDataDoubleEscaped;
                        }

                        self.emit_character_token(c)?;
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the temporary buffer. Emit the current
                    // input character as a character token.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.temporary_buffer.push(c.to_ascii_lowercase());
                        self.emit_character_token(c)?;
                    }
                    // ASCII lower alpha
                    // Append the current input character to the temporary buffer. Emit the
                    // current input character as a character token.
                    Some(c) if is_ascii_lower_alpha(c) => {
                        self.temporary_buffer.push(c);

                        self.emit_character_token(c)?;
                    }
                    // Anything else
                    // Reconsume in the script data double escaped state.
                    _ => {
                        self.reconsume_in_state(State::ScriptDataDoubleEscaped);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#before-attribute-name-state
            State::BeforeAttributeName => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.skip_next_lf(c);
                    }
                    // U+002F SOLIDUS (/)
                    // U+003E GREATER-THAN SIGN (>)
                    // EOF
                    // Reconsume in the after attribute name state.
                    Some('/') | Some('>') | None => {
                        self.reconsume_in_state(State::AfterAttributeName);
                    }
                    // U+003D EQUALS SIGN (=)
                    // This is an unexpected-equals-sign-before-attribute-name parse error.
                    // Start a new attribute in the current tag token. Set that attribute's name
                    // to the current input character, and its value to the empty string. Switch
                    // to the attribute name state.
                    // We set `None` for `value` to support boolean attributes in AST
                    Some(c @ '=') => {
                        self.emit_error(ErrorKind::UnexpectedEqualsSignBeforeAttributeName);
                        self.start_new_attribute();
                        self.append_name_to_attribute(c, Some(c));
                        self.state = State::AttributeName;
                    }
                    // Anything else
                    // Start a new attribute in the current tag token. Set that attribute name
                    // and value to the empty string. Reconsume in the attribute name state.
                    // We set `None` for `value` to support boolean attributes in AST
                    _ => {
                        self.start_new_attribute();
                        self.reconsume_in_state(State::AttributeName);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#attribute-name-state
            State::AttributeName => {
                let anything_else = |lexer: &mut Lexer<I>, c: char| {
                    lexer.append_name_to_attribute(c, Some(c));
                };

                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // U+002F SOLIDUS (/)
                    // U+003E GREATER-THAN SIGN (>)
                    // EOF
                    // Reconsume in the after attribute name state.
                    Some(c) if is_spacy(c) => {
                        self.update_attribute_span();
                        self.skip_next_lf(c);
                        self.reconsume_in_state(State::AfterAttributeName);
                    }
                    Some('/' | '>') | None => {
                        self.update_attribute_span();
                        self.reconsume_in_state(State::AfterAttributeName);
                    }
                    // U+003D EQUALS SIGN (=)
                    // Switch to the before attribute value state.
                    Some('=') => {
                        self.state = State::BeforeAttributeValue;
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the current attribute's name.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.append_name_to_attribute(c.to_ascii_lowercase(), Some(c));
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current attribute's name.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_name_to_attribute(REPLACEMENT_CHARACTER, Some(c));
                    }
                    // U+0022 QUOTATION MARK (")
                    // U+0027 APOSTROPHE (')
                    // U+003C LESS-THAN SIGN (<)
                    // This is an unexpected-character-in-attribute-name parse error. Treat it
                    // as per the "anything else" entry below.
                    Some(c @ '"') | Some(c @ '\'') | Some(c @ '<') => {
                        self.emit_error(ErrorKind::UnexpectedCharacterInAttributeName);

                        anything_else(self, c);
                    }
                    // Anything else
                    // Append the current input character to the current attribute's name.
                    Some(c) => {
                        self.validate_input_stream_character(c);

                        anything_else(self, c);
                    }
                }

                // When the user agent leaves the attribute name state (and
                // before emitting the tag token, if appropriate), the
                // complete attribute's name must be compared to the other
                // attributes on the same token; if there is already an
                // attribute on the token with the exact same name, then
                // this is a duplicate-attribute parse error and the new
                // attribute must be removed from the token.
                //
                // We postpone it when we will emit current tag token
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#after-attribute-name-state
            State::AfterAttributeName => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.skip_next_lf(c);
                    }
                    // U+002F SOLIDUS (/)
                    // Switch to the self-closing start tag state.
                    Some('/') => {
                        self.state = State::SelfClosingStartTag;
                    }
                    // U+003D EQUALS SIGN (=)
                    // Switch to the before attribute value state.
                    Some('=') => {
                        self.state = State::BeforeAttributeValue;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current tag token.
                    Some('>') => {
                        self.state = State::Data;
                        self.emit_tag_token();
                    }
                    // EOF
                    // This is an eof-in-tag parse error. Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInTag);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Start a new attribute in the current tag token. Set that attribute name
                    // and value to the empty string. Reconsume in the attribute name state.
                    // We set `None` for `value` to support boolean attributes in AST
                    _ => {
                        self.start_new_attribute();
                        self.reconsume_in_state(State::AttributeName);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#before-attribute-value-state
            State::BeforeAttributeValue => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.skip_next_lf(c);
                    }
                    // U+0022 QUOTATION MARK (")
                    // Switch to the attribute value (double-quoted) state.
                    Some(c @ '"') => {
                        self.append_value_to_attribute(true, None, Some(c));
                        self.state = State::AttributeValueDoubleQuoted;
                    }
                    // U+0027 APOSTROPHE (')
                    // Switch to the attribute value (single-quoted) state.
                    Some(c @ '\'') => {
                        self.append_value_to_attribute(true, None, Some(c));
                        self.state = State::AttributeValueSingleQuoted;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is a missing-attribute-value parse error. Switch to the data state.
                    // Emit the current tag token.
                    Some('>') => {
                        self.emit_error(ErrorKind::MissingAttributeValue);
                        self.state = State::Data;
                        self.emit_tag_token();
                    }
                    // Anything else
                    // Reconsume in the attribute value (unquoted) state.
                    _ => {
                        self.reconsume_in_state(State::AttributeValueUnquoted);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#attribute-value-(double-quoted)-state
            State::AttributeValueDoubleQuoted => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0022 QUOTATION MARK (")
                    // Switch to the after attribute value (quoted) state.
                    // We set value to support empty attributes (i.e. `attr=""`)
                    Some(c @ '"') => {
                        self.append_value_to_attribute(false, None, Some(c));
                        self.state = State::AfterAttributeValueQuoted;
                    }
                    // U+0026 AMPERSAND (&)
                    // Set the return state to the attribute value (double-quoted) state. Switch
                    // to the character reference state.
                    Some('&') => {
                        self.return_state = State::AttributeValueDoubleQuoted;
                        self.state = State::CharacterReference;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current attribute's value.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_value_to_attribute(false, Some(REPLACEMENT_CHARACTER), Some(c));
                    }
                    // EOF
                    // This is an eof-in-tag parse error. Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInTag);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current attribute's value.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_value_to_attribute(false, Some(c), Some(c));
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#attribute-value-(single-quoted)-state
            State::AttributeValueSingleQuoted => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0027 APOSTROPHE (')
                    // Switch to the after attribute value (quoted) state.
                    // We set value to support empty attributes (i.e. `attr=''`)
                    Some(c @ '\'') => {
                        self.append_value_to_attribute(false, None, Some(c));
                        self.state = State::AfterAttributeValueQuoted;
                    }
                    // U+0026 AMPERSAND (&)
                    // Set the return state to the attribute value (single-quoted) state. Switch
                    // to the character reference state.
                    Some('&') => {
                        self.return_state = State::AttributeValueSingleQuoted;
                        self.state = State::CharacterReference;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current attribute's value.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_value_to_attribute(false, Some(REPLACEMENT_CHARACTER), Some(c));
                    }
                    // EOF
                    // This is an eof-in-tag parse error. Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInTag);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current attribute's value.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_value_to_attribute(false, Some(c), Some(c));
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#attribute-value-(unquoted)-state
            State::AttributeValueUnquoted => {
                let anything_else = |lexer: &mut Lexer<I>, c: char| {
                    lexer.append_value_to_attribute(false, Some(c), Some(c));
                };

                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Switch to the before attribute name state.
                    Some(c) if is_spacy(c) => {
                        self.update_attribute_span();
                        self.skip_next_lf(c);
                        self.state = State::BeforeAttributeName;
                    }
                    // U+0026 AMPERSAND (&)
                    // Set the return state to the attribute value (unquoted) state. Switch to
                    // the character reference state.
                    Some('&') => {
                        self.return_state = State::AttributeValueUnquoted;
                        self.state = State::CharacterReference;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current tag token.
                    Some('>') => {
                        self.update_attribute_span();
                        self.state = State::Data;
                        self.emit_tag_token();
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current attribute's value.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_value_to_attribute(false, Some(REPLACEMENT_CHARACTER), Some(c));
                    }
                    // U+0022 QUOTATION MARK (")
                    // U+0027 APOSTROPHE (')
                    // U+003C LESS-THAN SIGN (<)
                    // U+003D EQUALS SIGN (=)
                    // U+0060 GRAVE ACCENT (`)
                    // This is an unexpected-character-in-unquoted-attribute-value parse error.
                    // Treat it as per the "anything else" entry below.
                    Some(c @ '"') | Some(c @ '\'') | Some(c @ '<') | Some(c @ '=')
                    | Some(c @ '`') => {
                        self.emit_error(ErrorKind::UnexpectedCharacterInUnquotedAttributeValue);

                        anything_else(self, c);
                    }
                    // EOF
                    // This is an eof-in-tag parse error. Emit an end-of-file token.
                    None => {
                        self.update_attribute_span();
                        self.emit_error(ErrorKind::EofInTag);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current attribute's value.
                    Some(c) => {
                        self.validate_input_stream_character(c);

                        anything_else(self, c);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#after-attribute-value-(quoted)-state
            State::AfterAttributeValueQuoted => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Switch to the before attribute name state.
                    Some(c) if is_spacy(c) => {
                        self.update_attribute_span();
                        self.skip_next_lf(c);
                        self.state = State::BeforeAttributeName;
                    }
                    // U+002F SOLIDUS (/)
                    // Switch to the self-closing start tag state.
                    Some('/') => {
                        self.update_attribute_span();
                        self.state = State::SelfClosingStartTag;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current tag token.
                    Some('>') => {
                        self.update_attribute_span();
                        self.state = State::Data;
                        self.emit_tag_token();
                    }
                    // EOF
                    // This is an eof-in-tag parse error. Emit an end-of-file token.
                    None => {
                        self.update_attribute_span();
                        self.emit_error(ErrorKind::EofInTag);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is a missing-whitespace-between-attributes parse error. Reconsume in
                    // the before attribute name state.
                    _ => {
                        self.update_attribute_span();
                        self.emit_error(ErrorKind::MissingWhitespaceBetweenAttributes);
                        self.reconsume_in_state(State::BeforeAttributeName);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#self-closing-start-tag-state
            State::SelfClosingStartTag => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+003E GREATER-THAN SIGN (>)
                    // Set the self-closing flag of the current tag token. Switch to the data
                    // state. Emit the current tag token.
                    Some('>') => {
                        if let Some(current_tag_token) = &mut self.current_tag_token {
                            current_tag_token.is_self_closing = true;
                        }

                        self.state = State::Data;
                        self.emit_tag_token();
                    }
                    // EOF
                    // This is an eof-in-tag parse error. Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInTag);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is an unexpected-solidus-in-tag parse error. Reconsume in the before
                    // attribute name state.
                    _ => {
                        self.emit_error(ErrorKind::UnexpectedSolidusInTag);
                        self.reconsume_in_state(State::BeforeAttributeName);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#bogus-comment-state
            State::BogusComment => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current comment token.
                    Some('>') => {
                        self.state = State::Data;
                        self.emit_comment_token(Some(">"));
                    }
                    // EOF
                    // Emit the comment. Emit an end-of-file token.
                    None => {
                        self.emit_comment_token(None);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the comment token's data.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_to_comment_token(REPLACEMENT_CHARACTER, c);
                    }
                    // Anything else
                    // Append the current input character to the comment token's data.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_append_to_comment_token(c);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#markup-declaration-open-state
            State::MarkupDeclarationOpen => {
                let cur_pos = self.input.cur_pos();
                let anything_else = |lexer: &mut Lexer<I>| {
                    lexer.emit_error(ErrorKind::IncorrectlyOpenedComment);
                    lexer.create_comment_token(None, "<!");
                    lexer.state = State::BogusComment;
                    lexer.cur_pos = cur_pos;
                    // We don't validate input here because we reset position
                    lexer.input.reset_to(cur_pos);
                };

                // If the next few characters are:
                match self.consume_next_char() {
                    // Two U+002D HYPHEN-MINUS characters (-)
                    // Consume those two characters, create a comment token whose data
                    // is the empty string, and switch to the comment start state.
                    Some('-') => match self.consume_next_char() {
                        Some('-') => {
                            self.create_comment_token(None, "<!--");
                            self.state = State::CommentStart;
                        }
                        _ => {
                            anything_else(self);
                        }
                    },
                    // ASCII case-insensitive match for the word "DOCTYPE"
                    // Consume those characters and switch to the DOCTYPE state.
                    Some(d @ 'd' | d @ 'D') => match self.consume_next_char() {
                        Some(o @ 'o' | o @ 'O') => match self.consume_next_char() {
                            Some(c @ 'c' | c @ 'C') => match self.consume_next_char() {
                                Some(t @ 't' | t @ 'T') => match self.consume_next_char() {
                                    Some(y @ 'y' | y @ 'Y') => match self.consume_next_char() {
                                        Some(p @ 'p' | p @ 'P') => match self.consume_next_char() {
                                            Some(e @ 'e' | e @ 'E') => {
                                                self.state = State::Doctype;

                                                let mut raw_keyword = String::with_capacity(9);

                                                raw_keyword.push('<');
                                                raw_keyword.push('!');
                                                raw_keyword.push(d);
                                                raw_keyword.push(o);
                                                raw_keyword.push(c);
                                                raw_keyword.push(t);
                                                raw_keyword.push(y);
                                                raw_keyword.push(p);
                                                raw_keyword.push(e);

                                                self.doctype_raw = Some(raw_keyword);
                                            }
                                            _ => {
                                                anything_else(self);
                                            }
                                        },
                                        _ => {
                                            anything_else(self);
                                        }
                                    },
                                    _ => {
                                        anything_else(self);
                                    }
                                },
                                _ => {
                                    anything_else(self);
                                }
                            },
                            _ => {
                                anything_else(self);
                            }
                        },
                        _ => {
                            anything_else(self);
                        }
                    },
                    // The string "[CDATA[" (the five uppercase letters "CDATA" with a
                    // U+005B LEFT SQUARE BRACKET character before and after)
                    // Consume those characters. If there is an adjusted current node and it
                    // is not an element in the HTML namespace, then switch to the CDATA
                    // section state. Otherwise, this is a cdata-in-html-content parse
                    // error. Create a comment token whose data is the "[CDATA[" string.
                    // Switch to the bogus comment state.
                    Some('[') => match self.consume_next_char() {
                        Some(c @ 'c' | c @ 'C') => match self.consume_next_char() {
                            Some(d @ 'd' | d @ 'D') => match self.consume_next_char() {
                                Some(a1 @ 'a' | a1 @ 'A') => match self.consume_next_char() {
                                    Some(t @ 't' | t @ 'T') => match self.consume_next_char() {
                                        Some(a2 @ 'a' | a2 @ 'A') => {
                                            match self.consume_next_char() {
                                                Some('[') => {
                                                    if let Some(false) = self.is_adjusted_current_node_is_element_in_html_namespace {
                                                        self.state = State::CdataSection;
                                                    } else {
                                                        self.emit_error(
                                                            ErrorKind::CdataInHtmlContent,
                                                        );
                                                        let mut data = String::with_capacity(7);

                                                        data.push('[');
                                                        data.push(c);
                                                        data.push(d);
                                                        data.push(a1);
                                                        data.push(t);
                                                        data.push(a2);
                                                        data.push('[');

                                                        self.create_comment_token(Some(data), "<!");
                                                        self.state = State::BogusComment;
                                                    }
                                                }
                                                _ => {
                                                    anything_else(self);
                                                }
                                            }
                                        }
                                        _ => {
                                            anything_else(self);
                                        }
                                    },
                                    _ => {
                                        anything_else(self);
                                    }
                                },
                                _ => {
                                    anything_else(self);
                                }
                            },
                            _ => {
                                anything_else(self);
                            }
                        },
                        _ => {
                            anything_else(self);
                        }
                    },
                    // Anything else
                    // This is an incorrectly-opened-comment parse error. Create a comment token
                    // whose data is the empty string. Switch to the bogus comment state (don't
                    // consume anything in the current state).
                    _ => {
                        anything_else(self);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-start-state
            State::CommentStart => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the comment start dash state.
                    Some('-') => {
                        self.state = State::CommentStartDash;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is an abrupt-closing-of-empty-comment parse error. Switch to the
                    // data state. Emit the current comment token.
                    Some('>') => {
                        self.emit_error(ErrorKind::AbruptClosingOfEmptyComment);
                        self.state = State::Data;
                        self.emit_comment_token(Some(">"));
                    }
                    // Anything else
                    // Reconsume in the comment state.
                    _ => {
                        self.reconsume_in_state(State::Comment);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-start-dash-state
            State::CommentStartDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the comment end state.
                    Some('-') => {
                        self.state = State::CommentEnd;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is an abrupt-closing-of-empty-comment parse error. Switch to the
                    // data state. Emit the current comment token.
                    Some('>') => {
                        self.emit_error(ErrorKind::AbruptClosingOfEmptyComment);
                        self.state = State::Data;
                        self.emit_comment_token(Some("->"));
                    }
                    // EOF
                    // This is an eof-in-comment parse error. Emit the current comment token.
                    // Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInComment);
                        self.emit_comment_token(None);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append a U+002D HYPHEN-MINUS character (-) to the comment token's data.
                    // Reconsume in the comment state.
                    _ => {
                        self.append_to_comment_token('-', '-');
                        self.reconsume_in_state(State::Comment);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-state
            State::Comment => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+003C LESS-THAN SIGN (<)
                    // Append the current input character to the comment token's data. Switch to
                    // the comment less-than sign state.
                    Some(c @ '<') => {
                        self.append_to_comment_token(c, c);
                        self.state = State::CommentLessThanSign;
                    }
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the comment end dash state.
                    Some('-') => {
                        self.state = State::CommentEndDash;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the comment token's data.
                    Some(c @ '\x00') => {
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_to_comment_token(REPLACEMENT_CHARACTER, c);
                    }
                    // EOF
                    // This is an eof-in-comment parse error. Emit the current comment token.
                    // Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInComment);
                        self.emit_comment_token(None);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the comment token's data.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_append_to_comment_token(c);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-less-than-sign-state
            State::CommentLessThanSign => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0021 EXCLAMATION MARK (!)
                    // Append the current input character to the comment token's data. Switch to
                    // the comment less-than sign bang state.
                    Some(c @ '!') => {
                        self.append_to_comment_token(c, c);
                        self.state = State::CommentLessThanSignBang;
                    }
                    // U+003C LESS-THAN SIGN (<)
                    // Append the current input character to the comment token's data.
                    Some(c @ '<') => {
                        self.append_to_comment_token(c, c);
                    }
                    // Anything else
                    // Reconsume in the comment state.
                    _ => {
                        self.reconsume_in_state(State::Comment);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-less-than-sign-bang-state
            State::CommentLessThanSignBang => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the comment less-than sign bang dash state.
                    Some('-') => {
                        self.state = State::CommentLessThanSignBangDash;
                    }
                    // Anything else
                    // Reconsume in the comment state.
                    _ => {
                        self.reconsume_in_state(State::Comment);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-less-than-sign-bang-dash-state
            State::CommentLessThanSignBangDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the comment less-than sign bang dash dash state.
                    Some('-') => {
                        self.state = State::CommentLessThanSignBangDashDash;
                    }
                    // Anything else
                    // Reconsume in the comment end dash state.
                    _ => {
                        self.reconsume_in_state(State::CommentEndDash);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-less-than-sign-bang-dash-dash-state
            State::CommentLessThanSignBangDashDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+003E GREATER-THAN SIGN (>)
                    // EOF
                    // Reconsume in the comment end state.
                    Some('>') | None => {
                        self.reconsume_in_state(State::CommentEnd);
                    }
                    // Anything else
                    // This is a nested-comment parse error. Reconsume in the comment end state.
                    _ => {
                        self.emit_error(ErrorKind::NestedComment);
                        self.reconsume_in_state(State::CommentEnd);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-end-dash-state
            State::CommentEndDash => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Switch to the comment end state.
                    Some('-') => {
                        self.state = State::CommentEnd;
                    }
                    // EOF
                    // This is an eof-in-comment parse error. Emit the current comment token.
                    // Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInComment);
                        self.emit_comment_token(None);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append a U+002D HYPHEN-MINUS character (-) to the comment token's data.
                    // Reconsume in the comment state.
                    _ => {
                        self.append_to_comment_token('-', '-');
                        self.reconsume_in_state(State::Comment);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-end-state
            State::CommentEnd => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current comment token.
                    Some('>') => {
                        self.state = State::Data;
                        self.emit_comment_token(Some("-->"));
                    }
                    // U+0021 EXCLAMATION MARK (!)
                    // Switch to the comment end bang state.
                    Some('!') => {
                        self.state = State::CommentEndBang;
                    }
                    // U+002D HYPHEN-MINUS (-)
                    // Append a U+002D HYPHEN-MINUS character (-) to the comment token's data.
                    Some(c @ '-') => {
                        self.append_to_comment_token(c, c);
                    }
                    // EOF
                    // This is an eof-in-comment parse error. Emit the current comment token.
                    // Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInComment);
                        self.emit_comment_token(None);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append two U+002D HYPHEN-MINUS characters (-) to the comment token's
                    // data. Reconsume in the comment state.
                    _ => {
                        self.append_to_comment_token('-', '-');
                        self.append_to_comment_token('-', '-');
                        self.reconsume_in_state(State::Comment);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#comment-end-bang-state
            State::CommentEndBang => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+002D HYPHEN-MINUS (-)
                    // Append two U+002D HYPHEN-MINUS characters (-) and a U+0021 EXCLAMATION
                    // MARK character (!) to the comment token's data. Switch to the comment end
                    // dash state.
                    Some(c @ '-') => {
                        self.append_to_comment_token(c, c);
                        self.append_to_comment_token('-', '-');
                        self.append_to_comment_token('!', '!');
                        self.state = State::CommentEndDash;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is an incorrectly-closed-comment parse error. Switch to the data
                    // state. Emit the current comment token.
                    Some('>') => {
                        self.emit_error(ErrorKind::IncorrectlyClosedComment);
                        self.state = State::Data;
                        self.emit_comment_token(Some(">"));
                    }
                    // EOF
                    // This is an eof-in-comment parse error. Emit the current comment token.
                    // Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInComment);
                        self.emit_comment_token(None);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append two U+002D HYPHEN-MINUS characters (-) and a U+0021 EXCLAMATION
                    // MARK character (!) to the comment token's data. Reconsume in the comment
                    // state.
                    _ => {
                        self.append_to_comment_token('-', '-');
                        self.append_to_comment_token('-', '-');
                        self.append_to_comment_token('!', '!');
                        self.reconsume_in_state(State::Comment);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#doctype-state
            State::Doctype => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Switch to the before DOCTYPE name state.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::BeforeDoctypeName;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Reconsume in the before DOCTYPE name state.
                    Some('>') => {
                        self.reconsume_in_state(State::BeforeDoctypeName);
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Create a new DOCTYPE token. Set
                    // its force-quirks flag to on. Emit the current token. Emit an end-of-file
                    // token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.create_doctype_token(None);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is a missing-whitespace-before-doctype-name parse error. Reconsume
                    // in the before DOCTYPE name state.
                    _ => {
                        self.emit_error(ErrorKind::MissingWhitespaceBeforeDoctypeName);
                        self.reconsume_in_state(State::BeforeDoctypeName);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#before-doctype-name-state
            State::BeforeDoctypeName => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                    }
                    // ASCII upper alpha
                    // Create a new DOCTYPE token. Set the token's name to the lowercase version
                    // of the current input character (add 0x0020 to the character's code
                    // point). Switch to the DOCTYPE name state.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.append_raw_to_doctype_token(c);
                        self.create_doctype_token(Some(c.to_ascii_lowercase()));
                        self.state = State::DoctypeName;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Create a new DOCTYPE
                    // token. Set the token's name to a U+FFFD REPLACEMENT CHARACTER character.
                    // Switch to the DOCTYPE name state.
                    Some(c @ '\x00') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.create_doctype_token(Some(REPLACEMENT_CHARACTER));
                        self.state = State::DoctypeName;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is a missing-doctype-name parse error. Create a new DOCTYPE token.
                    // Set its force-quirks flag to on. Switch to the data state. Emit the
                    // current token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::MissingDoctypeName);
                        self.create_doctype_token(None);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Create a new DOCTYPE token. Set
                    // its force-quirks flag to on. Emit the current token. Emit an end-of-file
                    // token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.create_doctype_token(None);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Create a new DOCTYPE token. Set the token's name to the current input
                    // character. Switch to the DOCTYPE name state.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_raw_to_doctype_token(c);
                        self.create_doctype_token(Some(c));
                        self.state = State::DoctypeName;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#doctype-name-state
            State::DoctypeName => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Switch to the after DOCTYPE name state.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::AfterDoctypeName;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // ASCII upper alpha
                    // Append the lowercase version of the current input character (add 0x0020
                    // to the character's code point) to the current DOCTYPE token's name.
                    Some(c) if is_ascii_upper_alpha(c) => {
                        self.append_raw_to_doctype_token(c);
                        self.append_to_doctype_token(Some(c.to_ascii_lowercase()), None, None);
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current DOCTYPE token's name.
                    Some(c @ '\x00') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_to_doctype_token(Some(REPLACEMENT_CHARACTER), None, None);
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current DOCTYPE token's name.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_raw_to_doctype_token(c);
                        self.append_to_doctype_token(Some(c), None, None);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#after-doctype-name-state
            State::AfterDoctypeName => {
                let cur_pos = self.input.cur_pos();

                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // If the six characters starting from the current input character are an
                    // ASCII case-insensitive match for the word "PUBLIC", then consume those
                    // characters and switch to the after DOCTYPE public keyword state.
                    //
                    // Otherwise, if the six characters starting from the current input
                    // character are an ASCII case-insensitive match for the word "SYSTEM", then
                    // consume those characters and switch to the after DOCTYPE system keyword
                    // state.
                    //
                    // Otherwise, this is an invalid-character-sequence-after-doctype-name parse
                    // error. Set the current DOCTYPE token's force-quirks flag to on. Reconsume
                    // in the bogus DOCTYPE state.
                    Some(c) => {
                        let mut first_six_chars = String::with_capacity(6);

                        first_six_chars.push(c);

                        for _ in 0..5 {
                            match self.consume_next_char() {
                                Some(c) => {
                                    first_six_chars.push(c);
                                }
                                _ => {
                                    break;
                                }
                            }
                        }

                        match &*first_six_chars.to_lowercase() {
                            "public" => {
                                self.state = State::AfterDoctypePublicKeyword;

                                if let Some(doctype_raw) = &mut self.doctype_raw {
                                    doctype_raw.push_str(&first_six_chars);
                                }
                            }
                            "system" => {
                                self.state = State::AfterDoctypeSystemKeyword;

                                if let Some(doctype_raw) = &mut self.doctype_raw {
                                    doctype_raw.push_str(&first_six_chars);
                                }
                            }
                            _ => {
                                self.cur_pos = cur_pos;
                                self.input.reset_to(cur_pos);
                                self.emit_error(
                                    ErrorKind::InvalidCharacterSequenceAfterDoctypeName,
                                );
                                self.set_force_quirks();
                                self.reconsume_in_state(State::BogusDoctype);
                            }
                        }
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#after-doctype-public-keyword-state
            State::AfterDoctypePublicKeyword => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Switch to the before DOCTYPE public identifier state.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::BeforeDoctypePublicIdentifier;
                    }
                    // U+0022 QUOTATION MARK (")
                    // This is a missing-whitespace-after-doctype-public-keyword parse error.
                    // Set the current DOCTYPE token's public identifier to the empty string
                    // (not missing), then switch to the DOCTYPE public identifier
                    // (double-quoted) state.
                    Some(c @ '"') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::MissingWhitespaceAfterDoctypePublicKeyword);
                        self.set_doctype_token_public_id();
                        self.state = State::DoctypePublicIdentifierDoubleQuoted;
                    }
                    // U+0027 APOSTROPHE (')
                    // This is a missing-whitespace-after-doctype-public-keyword parse error.
                    // Set the current DOCTYPE token's public identifier to the empty string
                    // (not missing), then switch to the DOCTYPE public identifier
                    // (single-quoted) state.
                    Some(c @ '\'') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::MissingWhitespaceAfterDoctypePublicKeyword);
                        self.set_doctype_token_public_id();
                        self.state = State::DoctypePublicIdentifierSingleQuoted;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is a missing-doctype-public-identifier parse error. Set the current
                    // DOCTYPE token's force-quirks flag to on. Switch to the data state. Emit
                    // the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::MissingDoctypePublicIdentifier);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is a missing-quote-before-doctype-public-identifier parse error. Set
                    // the current DOCTYPE token's force-quirks flag to on. Reconsume in the
                    // bogus DOCTYPE state.
                    _ => {
                        self.emit_error(ErrorKind::MissingQuoteBeforeDoctypePublicIdentifier);
                        self.set_force_quirks();
                        self.reconsume_in_state(State::BogusDoctype);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#before-doctype-public-identifier-state
            State::BeforeDoctypePublicIdentifier => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                    }
                    // U+0022 QUOTATION MARK (")
                    // Set the current DOCTYPE token's public identifier to the empty string
                    // (not missing), then switch to the DOCTYPE public identifier
                    // (double-quoted) state.
                    Some(c @ '"') => {
                        self.append_raw_to_doctype_token(c);
                        self.set_doctype_token_public_id();
                        self.state = State::DoctypePublicIdentifierDoubleQuoted;
                    }
                    // U+0027 APOSTROPHE (')
                    // Set the current DOCTYPE token's public identifier to the empty string
                    // (not missing), then switch to the DOCTYPE public identifier
                    // (single-quoted) state.
                    Some(c @ '\'') => {
                        self.append_raw_to_doctype_token(c);
                        self.set_doctype_token_public_id();
                        self.state = State::DoctypePublicIdentifierSingleQuoted;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is a missing-doctype-public-identifier parse error. Set the current
                    // DOCTYPE token's force-quirks flag to on. Switch to the data state. Emit
                    // the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::MissingDoctypePublicIdentifier);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is a missing-quote-before-doctype-public-identifier parse error. Set
                    // the current DOCTYPE token's force-quirks flag to on. Reconsume in the
                    // bogus DOCTYPE state.
                    _ => {
                        self.emit_error(ErrorKind::MissingQuoteBeforeDoctypePublicIdentifier);
                        self.set_force_quirks();
                        self.reconsume_in_state(State::BogusDoctype);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#doctype-public-identifier-(double-quoted)-state
            State::DoctypePublicIdentifierDoubleQuoted => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0022 QUOTATION MARK (")
                    // Switch to the after DOCTYPE public identifier state.
                    Some(c @ '"') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::AfterDoctypePublicIdentifier;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current DOCTYPE token's public
                    // identifier.
                    Some(c @ '\x00') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_to_doctype_token(None, Some(REPLACEMENT_CHARACTER), None);
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is an abrupt-doctype-public-identifier parse error. Set the current
                    // DOCTYPE token's force-quirks flag to on. Switch to the data state. Emit
                    // the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::AbruptDoctypePublicIdentifier);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current DOCTYPE token's public
                    // identifier.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_raw_to_doctype_token(c);
                        self.append_to_doctype_token(None, Some(c), None);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#doctype-public-identifier-(single-quoted)-state
            State::DoctypePublicIdentifierSingleQuoted => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0027 APOSTROPHE (')
                    // Switch to the after DOCTYPE public identifier state.
                    Some(c @ '\'') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::AfterDoctypePublicIdentifier;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current DOCTYPE token's public
                    // identifier.
                    Some(c @ '\x00') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_to_doctype_token(None, Some(REPLACEMENT_CHARACTER), None);
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is an abrupt-doctype-public-identifier parse error. Set the current
                    // DOCTYPE token's force-quirks flag to on. Switch to the data state. Emit
                    // the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::AbruptDoctypePublicIdentifier);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current DOCTYPE token's public
                    // identifier.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_raw_to_doctype_token(c);
                        self.append_to_doctype_token(None, Some(c), None);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#after-doctype-public-identifier-state
            State::AfterDoctypePublicIdentifier => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Switch to the between DOCTYPE public and system identifiers state.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::BetweenDoctypePublicAndSystemIdentifiers;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // U+0022 QUOTATION MARK (")
                    // This is a missing-whitespace-between-doctype-public-and-system-identifiers
                    // parse error. Set the current DOCTYPE token's system
                    // identifier to the empty string (not missing), then switch
                    // to the DOCTYPE system identifier (double-quoted) state.
                    Some(c @ '"') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(
                            ErrorKind::MissingWhitespaceBetweenDoctypePublicAndSystemIdentifiers,
                        );
                        self.set_doctype_token_system_id();
                        self.state = State::DoctypeSystemIdentifierDoubleQuoted;
                    }
                    // U+0027 APOSTROPHE (')
                    // This is a missing-whitespace-between-doctype-public-and-system-identifiers
                    // parse error. Set the current DOCTYPE token's system
                    // identifier to the empty string (not missing), then switch
                    // to the DOCTYPE system identifier (single-quoted) state.
                    Some(c @ '\'') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(
                            ErrorKind::MissingWhitespaceBetweenDoctypePublicAndSystemIdentifiers,
                        );
                        self.set_doctype_token_system_id();
                        self.state = State::DoctypeSystemIdentifierSingleQuoted;
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is a missing-quote-before-doctype-system-identifier parse error. Set
                    // the current DOCTYPE token's force-quirks flag to on. Reconsume in the
                    // bogus DOCTYPE state.
                    _ => {
                        self.emit_error(ErrorKind::MissingQuoteBeforeDoctypeSystemIdentifier);
                        self.set_force_quirks();
                        self.reconsume_in_state(State::BogusDoctype);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#between-doctype-public-and-system-identifiers-state
            State::BetweenDoctypePublicAndSystemIdentifiers => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // U+0022 QUOTATION MARK (")
                    // Set the current DOCTYPE token's system identifier to the empty string
                    // (not missing), then switch to the DOCTYPE system identifier
                    // (double-quoted) state.
                    Some(c @ '"') => {
                        self.append_raw_to_doctype_token(c);
                        self.set_doctype_token_system_id();
                        self.state = State::DoctypeSystemIdentifierDoubleQuoted;
                    }
                    // U+0027 APOSTROPHE (')
                    // Set the current DOCTYPE token's system identifier to the empty string
                    // (not missing), then switch to the DOCTYPE system identifier
                    // (single-quoted) state.
                    Some(c @ '\'') => {
                        self.append_raw_to_doctype_token(c);
                        self.set_doctype_token_system_id();
                        self.state = State::DoctypeSystemIdentifierSingleQuoted;
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is a missing-quote-before-doctype-system-identifier parse error. Set
                    // the current DOCTYPE token's force-quirks flag to on. Reconsume in the
                    // bogus DOCTYPE state
                    _ => {
                        self.emit_error(ErrorKind::MissingQuoteBeforeDoctypeSystemIdentifier);
                        self.set_force_quirks();
                        self.reconsume_in_state(State::BogusDoctype);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#after-doctype-system-keyword-state
            State::AfterDoctypeSystemKeyword => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Switch to the before DOCTYPE system identifier state.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::BeforeDoctypeSystemIdentifier;
                    }
                    // U+0022 QUOTATION MARK (")
                    // This is a missing-whitespace-after-doctype-system-keyword parse error.
                    // Set the current DOCTYPE token's system identifier to the empty string
                    // (not missing), then switch to the DOCTYPE system identifier
                    // (double-quoted) state.
                    Some(c @ '"') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::MissingWhitespaceAfterDoctypeSystemKeyword);
                        self.set_doctype_token_system_id();
                        self.state = State::DoctypeSystemIdentifierDoubleQuoted;
                    }
                    // U+0027 APOSTROPHE (')
                    // This is a missing-whitespace-after-doctype-system-keyword parse error.
                    // Set the current DOCTYPE token's system identifier to the empty string
                    // (not missing), then switch to the DOCTYPE system identifier
                    // (single-quoted) state.
                    Some(c @ '\'') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::MissingWhitespaceAfterDoctypeSystemKeyword);
                        self.set_doctype_token_system_id();
                        self.state = State::DoctypeSystemIdentifierSingleQuoted;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is a missing-doctype-system-identifier parse error. Set the current
                    // DOCTYPE token's force-quirks flag to on. Switch to the data state. Emit
                    // the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::MissingDoctypeSystemIdentifier);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is a missing-quote-before-doctype-system-identifier parse error. Set
                    // the current DOCTYPE token's force-quirks flag to on. Reconsume in the
                    // bogus DOCTYPE state.
                    _ => {
                        self.emit_error(ErrorKind::MissingQuoteBeforeDoctypeSystemIdentifier);
                        self.set_force_quirks();
                        self.reconsume_in_state(State::BogusDoctype);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#before-doctype-system-identifier-state
            State::BeforeDoctypeSystemIdentifier => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                    }
                    // U+0022 QUOTATION MARK (")
                    // Set the current DOCTYPE token's system identifier to the empty string
                    // (not missing), then switch to the DOCTYPE system identifier
                    // (double-quoted) state.
                    Some(c @ '"') => {
                        self.append_raw_to_doctype_token(c);
                        self.set_doctype_token_system_id();
                        self.state = State::DoctypeSystemIdentifierDoubleQuoted;
                    }
                    // U+0027 APOSTROPHE (')
                    // Set the current DOCTYPE token's system identifier to the empty string
                    // (not missing), then switch to the DOCTYPE system identifier
                    // (single-quoted) state.
                    Some(c @ '\'') => {
                        self.append_raw_to_doctype_token(c);
                        self.set_doctype_token_system_id();
                        self.state = State::DoctypeSystemIdentifierSingleQuoted;
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is a missing-doctype-system-identifier parse error. Set the current
                    // DOCTYPE token's force-quirks flag to on. Switch to the data state. Emit
                    // the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is a missing-quote-before-doctype-system-identifier parse error. Set
                    // the current DOCTYPE token's force-quirks flag to on. Reconsume in the
                    // bogus DOCTYPE state.
                    _ => {
                        self.emit_error(ErrorKind::MissingQuoteBeforeDoctypeSystemIdentifier);
                        self.set_force_quirks();
                        self.reconsume_in_state(State::BogusDoctype);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#doctype-system-identifier-(double-quoted)-state
            State::DoctypeSystemIdentifierDoubleQuoted => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0027 APOSTROPHE (')
                    // Switch to the after DOCTYPE system identifier state.
                    Some(c @ '"') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::AfterDoctypeSystemIdentifier;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current DOCTYPE token's system
                    // identifier.
                    Some(c @ '\x00') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_to_doctype_token(None, None, Some(REPLACEMENT_CHARACTER));
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is an abrupt-doctype-system-identifier parse error. Set the current
                    // DOCTYPE token's force-quirks flag to on. Switch to the data state. Emit
                    // the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::AbruptDoctypeSystemIdentifier);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current DOCTYPE token's system
                    // identifier.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_raw_to_doctype_token(c);
                        self.append_to_doctype_token(None, None, Some(c));
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#doctype-system-identifier-(single-quoted)-state
            State::DoctypeSystemIdentifierSingleQuoted => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0027 APOSTROPHE (')
                    // Switch to the after DOCTYPE system identifier state.
                    Some(c @ '\'') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::AfterDoctypeSystemIdentifier;
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Append a U+FFFD
                    // REPLACEMENT CHARACTER character to the current DOCTYPE token's system
                    // identifier.
                    Some(c @ '\x00') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                        self.append_to_doctype_token(None, None, Some(REPLACEMENT_CHARACTER));
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // This is an abrupt-doctype-system-identifier parse error. Set the current
                    // DOCTYPE token's force-quirks flag to on. Switch to the data state. Emit
                    // the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::AbruptDoctypeSystemIdentifier);
                        self.set_force_quirks();
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Append the current input character to the current DOCTYPE token's system
                    // identifier.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_raw_to_doctype_token(c);
                        self.append_to_doctype_token(None, None, Some(c));
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#after-doctype-system-identifier-state
            State::AfterDoctypeSystemIdentifier => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0009 CHARACTER TABULATION (tab)
                    // U+000A LINE FEED (LF)
                    // U+000C FORM FEED (FF)
                    // U+0020 SPACE
                    // Ignore the character.
                    Some(c) if is_spacy(c) => {
                        self.append_raw_to_doctype_token(c);
                    }
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the current DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // EOF
                    // This is an eof-in-doctype parse error. Set the current DOCTYPE token's
                    // force-quirks flag to on. Emit the current DOCTYPE token. Emit an
                    // end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInDoctype);
                        self.set_force_quirks();
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // This is an unexpected-character-after-doctype-system-identifier parse
                    // error. Reconsume in the bogus DOCTYPE state. (This does not set the
                    // current DOCTYPE token's force-quirks flag to on.)
                    _ => {
                        self.emit_error(ErrorKind::UnexpectedCharacterAfterDoctypeSystemIdentifier);
                        self.reconsume_in_state(State::BogusDoctype);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#bogus-doctype-state
            State::BogusDoctype => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+003E GREATER-THAN SIGN (>)
                    // Switch to the data state. Emit the DOCTYPE token.
                    Some(c @ '>') => {
                        self.append_raw_to_doctype_token(c);
                        self.state = State::Data;
                        self.emit_doctype_token();
                    }
                    // U+0000 NULL
                    // This is an unexpected-null-character parse error. Ignore the character.
                    Some(c @ '\x00') => {
                        self.append_raw_to_doctype_token(c);
                        self.emit_error(ErrorKind::UnexpectedNullCharacter);
                    }
                    // EOF
                    // Emit the DOCTYPE token. Emit an end-of-file token.
                    None => {
                        self.emit_doctype_token();
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Ignore the character.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.append_raw_to_doctype_token(c);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#cdata-section-state
            State::CdataSection => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+005D RIGHT SQUARE BRACKET (])
                    // Switch to the CDATA section bracket state.
                    Some(']') => {
                        self.state = State::CdataSectionBracket;
                    }
                    // EOF
                    // This is an eof-in-cdata parse error. Emit an end-of-file token.
                    None => {
                        self.emit_error(ErrorKind::EofInCdata);
                        self.emit_token(Token::Eof);

                        return Ok(());
                    }
                    // Anything else
                    // Emit the current input character as a character token.
                    Some(c) => {
                        self.validate_input_stream_character(c);
                        self.handle_raw_and_emit_character_token(c)?;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#cdata-section-bracket-state
            State::CdataSectionBracket => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+005D RIGHT SQUARE BRACKET (])
                    // Switch to the CDATA section end state.
                    Some(']') => {
                        self.state = State::CdataSectionEnd;
                    }
                    // Anything else
                    // Emit a U+005D RIGHT SQUARE BRACKET character token. Reconsume in the
                    // CDATA section state.
                    _ => {
                        self.emit_character_token(']')?;
                        self.reconsume_in_state(State::CdataSection);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#cdata-section-end-state
            State::CdataSectionEnd => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // U+005D RIGHT SQUARE BRACKET (])
                    // Emit a U+005D RIGHT SQUARE BRACKET character token.
                    Some(c @ ']') => {
                        self.emit_character_token_with_raw((']', c))?;
                    }
                    // U+003E GREATER-THAN SIGN character
                    // Switch to the data state.
                    Some('>') => {
                        self.state = State::Data;
                    }
                    // Anything else
                    // Emit two U+005D RIGHT SQUARE BRACKET character tokens. Reconsume in the
                    // CDATA section state.
                    _ => {
                        self.emit_character_token(']')?;
                        self.emit_character_token(']')?;
                        self.reconsume_in_state(State::CdataSection);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#character-reference-state
            State::CharacterReference => {
                // Set the temporary buffer to the empty string. Append a U+0026 AMPERSAND (&)
                // character to the temporary buffer.
                self.temporary_buffer.clear();
                self.temporary_buffer.push('&');

                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII alphanumeric
                    // Reconsume in the named character reference state.
                    Some(c) if c.is_ascii_alphanumeric() => {
                        self.reconsume_in_state(State::NamedCharacterReference);
                    }
                    // U+0023 NUMBER SIGN (#)
                    // Append the current input character to the temporary buffer. Switch to the
                    // numeric character reference state.
                    Some(c @ '#') => {
                        self.temporary_buffer.push(c);

                        self.state = State::NumericCharacterReference;
                    }
                    // Anything else
                    // Flush code points consumed as a character reference. Reconsume in the
                    // return state.
                    _ => {
                        self.flush_code_points_consumed_as_character_reference(None);
                        self.reconsume_in_state(self.return_state.clone());
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#named-character-reference-state
            State::NamedCharacterReference => {
                // Consume the maximum number of characters possible, where the consumed
                // characters are one of the identifiers in the first column of the named
                // character references table. Append each character to the temporary buffer
                // when it's consumed.
                // The shortest entity - `&GT`
                // The longest entity - `&CounterClockwiseContourIntegral;`
                let initial_cur_pos = self.input.cur_pos();

                let mut entity: Option<&Entity> = None;
                let mut entity_cur_pos: Option<BytePos> = None;
                let mut entity_temporary_buffer =
                    String::with_capacity(self.temporary_buffer.capacity());

                entity_temporary_buffer.push_str(&self.temporary_buffer);

                // No need to validate input, because we reset position if nothing was found
                while let Some(c) = &self.consume_next_char() {
                    entity_temporary_buffer.push(*c);

                    if let Some(found_entity) = HTML_ENTITIES.get(&entity_temporary_buffer) {
                        entity = Some(found_entity);
                        entity_cur_pos = Some(self.input.cur_pos());

                        self.temporary_buffer
                            .replace_range(1.., &entity_temporary_buffer[1..]);
                    } else {
                        // We stop when:
                        //
                        // - not ascii alphanumeric
                        // - we consume more characters than the longest entity
                        if !c.is_ascii_alphanumeric() || entity_temporary_buffer.len() > 32 {
                            break;
                        }
                    }
                }

                if entity.is_some() {
                    self.cur_pos = entity_cur_pos.unwrap();
                    self.input.reset_to(entity_cur_pos.unwrap());
                } else {
                    self.cur_pos = initial_cur_pos;
                    self.input.reset_to(initial_cur_pos);
                }

                let is_last_semicolon = self.temporary_buffer.ends_with(';');

                // If there is a match
                match entity {
                    Some(entity) => {
                        let is_next_equals_sign_or_ascii_alphanumeric = match self.next() {
                            Some('=') => true,
                            Some(c) if c.is_ascii_alphanumeric() => true,
                            _ => false,
                        };

                        // If the character reference was consumed as part of an attribute, and
                        // the last character matched is not a
                        // U+003B SEMICOLON character (;), and the next input
                        // character is either a U+003D EQUALS SIGN character (=) or an ASCII
                        // alphanumeric, then, for historical reasons, flush code points
                        // consumed as a character reference and
                        // switch to the return state.
                        if self.is_consumed_as_part_of_an_attribute()
                            && !is_last_semicolon
                            && is_next_equals_sign_or_ascii_alphanumeric
                        {
                            self.flush_code_points_consumed_as_character_reference(None);
                            self.state = self.return_state.clone();
                        }
                        // Otherwise:
                        //
                        // If the last character matched is not a U+003B SEMICOLON character
                        // (;), then this is a missing-semicolon-after-character-reference parse
                        // error.
                        //
                        // Set the temporary buffer to the empty string. Append one or two
                        // characters corresponding to the character reference name (as given by
                        // the second column of the named character references table) to the
                        // temporary buffer.
                        //
                        // Flush code points consumed as a character reference. Switch to the
                        // return state.
                        else {
                            if !is_last_semicolon {
                                self.emit_error(ErrorKind::MissingSemicolonAfterCharacterReference);
                            }

                            let old_temporary_buffer = self.temporary_buffer.clone();

                            self.temporary_buffer.clear();
                            self.temporary_buffer.push_str(&entity.characters);
                            self.flush_code_points_consumed_as_character_reference(Some(
                                old_temporary_buffer,
                            ));
                            self.state = self.return_state.clone();
                        }
                    }
                    // Otherwise
                    // Flush code points consumed as a character reference. Switch to the
                    // ambiguous ampersand state.
                    _ => {
                        self.flush_code_points_consumed_as_character_reference(None);
                        self.state = State::AmbiguousAmpersand;
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#ambiguous-ampersand-state
            State::AmbiguousAmpersand => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII alphanumeric
                    // If the character reference was consumed as part of an attribute, then
                    // append the current input character to the current attribute's value.
                    // Otherwise, emit the current input character as a character token.
                    Some(c) if c.is_ascii_alphanumeric() => {
                        if self.is_consumed_as_part_of_an_attribute() {
                            self.append_value_to_attribute(false, Some(c), Some(c));
                        } else {
                            self.emit_character_token(c)?;
                        }
                    }
                    // U+003B SEMICOLON (;)
                    // This is an unknown-named-character-reference parse error. Reconsume in
                    // the return state.
                    Some(';') => {
                        self.emit_error(ErrorKind::UnknownNamedCharacterReference);
                        self.reconsume_in_state(self.return_state.clone());
                    }
                    // Anything else
                    // Reconsume in the return state.
                    _ => {
                        self.reconsume_in_state(self.return_state.clone());
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#numeric-character-reference-state
            State::NumericCharacterReference => {
                self.character_reference_code = Some(vec![(0, 0, None)]);

                // Consume the next input character:
                match self.consume_next_char() {
                    // U+0078 LATIN SMALL LETTER X
                    // U+0058 LATIN CAPITAL LETTER X
                    // Append the current input character to the temporary buffer. Switch to the
                    // hexadecimal character reference start state.
                    Some(c @ 'x' | c @ 'X') => {
                        self.temporary_buffer.push(c);
                        self.state = State::HexademicalCharacterReferenceStart;
                    }
                    // Anything else
                    // Reconsume in the decimal character reference start state.
                    _ => {
                        self.reconsume_in_state(State::DecimalCharacterReferenceStart);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#hexadecimal-character-reference-start-state
            State::HexademicalCharacterReferenceStart => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII hex digit
                    // Reconsume in the hexadecimal character reference state.
                    Some(c) if is_ascii_hex_digit(c) => {
                        self.reconsume_in_state(State::HexademicalCharacterReference);
                    }
                    // Anything else
                    // This is an absence-of-digits-in-numeric-character-reference parse error.
                    // Flush code points consumed as a character reference. Reconsume in the
                    // return state.
                    _ => {
                        self.emit_error(ErrorKind::AbsenceOfDigitsInNumericCharacterReference);
                        self.flush_code_points_consumed_as_character_reference(None);
                        self.reconsume_in_state(self.return_state.clone());
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#decimal-character-reference-start-state
            State::DecimalCharacterReferenceStart => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII digit
                    // Reconsume in the decimal character reference state.
                    Some(c) if c.is_ascii_digit() => {
                        self.reconsume_in_state(State::DecimalCharacterReference);
                    }
                    // Anything else
                    // This is an absence-of-digits-in-numeric-character-reference parse error.
                    // Flush code points consumed as a character reference. Reconsume in the
                    // return state.
                    _ => {
                        self.emit_error(ErrorKind::AbsenceOfDigitsInNumericCharacterReference);
                        self.flush_code_points_consumed_as_character_reference(None);
                        self.reconsume_in_state(self.return_state.clone());
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#hexadecimal-character-reference-state
            State::HexademicalCharacterReference => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII digit
                    // Multiply the character reference code by 16. Add a numeric version of the
                    // current input character (subtract 0x0030 from the character's code point)
                    // to the character reference code.
                    Some(c) if c.is_ascii_digit() => match &mut self.character_reference_code {
                        Some(character_reference_code) => {
                            character_reference_code.push((16, c as u32 - 0x30, Some(c)));
                        }
                        _ => {
                            unreachable!();
                        }
                    },
                    // ASCII upper hex digit
                    // Multiply the character reference code by 16. Add a numeric version of the
                    // current input character as a hexadecimal digit (subtract 0x0037 from the
                    // character's code point) to the character reference code.
                    Some(c) if is_upper_hex_digit(c) => match &mut self.character_reference_code {
                        Some(character_reference_code) => {
                            character_reference_code.push((16, c as u32 - 0x37, Some(c)));
                        }
                        _ => {
                            unreachable!();
                        }
                    },
                    // ASCII lower hex digit
                    // Multiply the character reference code by 16. Add a numeric version of the
                    // current input character as a hexadecimal digit (subtract 0x0057 from the
                    // character's code point) to the character reference code.
                    Some(c) if is_lower_hex_digit(c) => match &mut self.character_reference_code {
                        Some(character_reference_code) => {
                            character_reference_code.push((16, c as u32 - 0x57, Some(c)));
                        }
                        _ => {
                            unreachable!();
                        }
                    },
                    // U+003B SEMICOLON
                    // Switch to the numeric character reference end state.
                    Some(';') => {
                        self.state = State::NumericCharacterReferenceEnd;
                    }
                    // Anything else
                    // This is a missing-semicolon-after-character-reference parse error.
                    // Reconsume in the numeric character reference end state.
                    _ => {
                        self.emit_error(ErrorKind::MissingSemicolonAfterCharacterReference);
                        self.reconsume_in_state(State::NumericCharacterReferenceEnd);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#decimal-character-reference-state
            State::DecimalCharacterReference => {
                // Consume the next input character:
                match self.consume_next_char() {
                    // ASCII digit
                    // Multiply the character reference code by 10. Add a numeric version of the
                    // current input character (subtract 0x0030 from the character's code point)
                    // to the character reference code.
                    Some(c) if c.is_ascii_digit() => match &mut self.character_reference_code {
                        Some(character_reference_code) => {
                            character_reference_code.push((10, c as u32 - 0x30, Some(c)));
                        }
                        _ => {
                            unreachable!();
                        }
                    },
                    // U+003B SEMICOLON
                    // Switch to the numeric character reference end state.
                    Some(';') => self.state = State::NumericCharacterReferenceEnd,
                    // Anything else
                    // This is a missing-semicolon-after-character-reference parse error.
                    // Reconsume in the numeric character reference end state.
                    _ => {
                        self.emit_error(ErrorKind::MissingSemicolonAfterCharacterReference);
                        self.reconsume_in_state(State::NumericCharacterReferenceEnd);
                    }
                }
            }
            // https://html.spec.whatwg.org/multipage/parsing.html#numeric-character-reference-end-state
            State::NumericCharacterReferenceEnd => {
                let (value, raw_char_ref) =
                    if let Some(chars) = self.character_reference_code.take() {
                        let mut raw = String::with_capacity(8);
                        let mut i: u32 = 0;
                        let mut overflowed = false;

                        for (base, value, c) in chars.iter() {
                            if let Some(c) = c {
                                raw.push(*c);
                            }

                            if !overflowed {
                                if let Some(result) = i.checked_mul(*base as u32) {
                                    i = result;

                                    if let Some(result) = i.checked_add(*value) {
                                        i = result;
                                    } else {
                                        i = 0x110000;

                                        overflowed = true;
                                    }
                                } else {
                                    i = 0x110000;

                                    overflowed = true;
                                }
                            }
                        }

                        (i, raw)
                    } else {
                        unreachable!();
                    };

                // Check the character reference code:
                let cr = match value {
                    // If the number is 0x00, then this is a null-character-reference
                    // parse error. Set the character
                    // reference code to 0xFFFD.
                    0 => {
                        self.emit_error(ErrorKind::NullCharacterReference);

                        0xfffd
                    }
                    // If the number is greater than 0x10FFFF, then this is a
                    // character-reference-outside-unicode-range parse error. Set the
                    // character reference code to
                    // 0xFFFD.
                    cr if cr > 0x10ffff => {
                        self.emit_error(ErrorKind::CharacterReferenceOutsideUnicodeRange);

                        0xfffd
                    }
                    // If the number is a surrogate, then this is a
                    // surrogate-character-reference parse error. Set the character
                    // reference code to 0xFFFD.
                    cr if is_surrogate(cr) => {
                        self.emit_error(ErrorKind::SurrogateCharacterReference);

                        0xfffd
                    }
                    // If the number is a noncharacter, then this is a
                    // noncharacter-character-reference parse error.
                    cr if is_noncharacter(cr) => {
                        self.emit_error(ErrorKind::NoncharacterCharacterReference);

                        cr
                    }
                    // If the number is 0x0D, or a control that's not ASCII whitespace,
                    // then
                    // this is a control-character-reference parse error. If the number
                    // is one of the numbers in the
                    // first column of the following table, then find the
                    // row with that number in the first column, and set the character
                    // reference code to the number in
                    // the second column of that row.
                    cr if cr == 0x0d || is_control(cr) => {
                        self.emit_error(ErrorKind::ControlCharacterReference);

                        match cr {
                            // 0x80	0x20AC	EURO SIGN (€)
                            0x80 => 0x20ac,
                            // 0x82	0x201A	SINGLE LOW-9 QUOTATION MARK (‚)
                            0x82 => 0x201a,
                            // 0x83	0x0192	LATIN SMALL LETTER F WITH HOOK (ƒ)
                            0x83 => 0x0192,
                            // 0x84	0x201E	DOUBLE LOW-9 QUOTATION MARK („)
                            0x84 => 0x201e,
                            // 0x85	0x2026	HORIZONTAL ELLIPSIS (…)
                            0x85 => 0x2026,
                            // 0x86	0x2020	DAGGER (†)
                            0x86 => 0x2020,
                            // 0x87	0x2021	DOUBLE DAGGER (‡)
                            0x87 => 0x2021,
                            // 0x88	0x02C6	MODIFIER LETTER CIRCUMFLEX ACCENT (ˆ)
                            0x88 => 0x02c6,
                            // 0x89	0x2030	PER MILLE SIGN (‰)
                            0x89 => 0x2030,
                            // 0x8A	0x0160	LATIN CAPITAL LETTER S WITH CARON (Š)
                            0x8a => 0x0160,
                            // 0x8B	0x2039	SINGLE LEFT-POINTING ANGLE QUOTATION MARK (‹)
                            0x8b => 0x2039,
                            // 0x8C	0x0152	LATIN CAPITAL LIGATURE OE (Œ)
                            0x8c => 0x0152,
                            // 0x8E	0x017D	LATIN CAPITAL LETTER Z WITH CARON (Ž)
                            0x8e => 0x017d,
                            // 0x91	0x2018	LEFT SINGLE QUOTATION MARK (‘)
                            0x91 => 0x2018,
                            // 0x92	0x2018	RIGHT SINGLE QUOTATION MARK (’)
                            0x92 => 0x2019,
                            // 0x93	0x201C	LEFT DOUBLE QUOTATION MARK (“)
                            0x93 => 0x201c,
                            // 0x94	0x201D	RIGHT DOUBLE QUOTATION MARK (”)
                            0x94 => 0x201d,
                            // 0x95	0x2022	BULLET (•)
                            0x95 => 0x2022,
                            // 0x96	0x2013	EN DASH (–)
                            0x96 => 0x2013,
                            // 0x97	0x2014	EM DASH (—)
                            0x97 => 0x2014,
                            // 0x98	0x02DC	SMALL TILDE (˜)
                            0x98 => 0x02dc,
                            // 0x99	0x2122	TRADE MARK SIGN (™)
                            0x99 => 0x2122,
                            // 0x9A	0x0161	LATIN SMALL LETTER S WITH CARON (š)
                            0x9a => 0x0161,
                            // 0x9B	0x203A	SINGLE RIGHT-POINTING ANGLE QUOTATION MARK (›)
                            0x9b => 0x203a,
                            // 0x9C	0x0153	LATIN SMALL LIGATURE OE (œ)
                            0x9c => 0x0153,
                            // 0x9E	0x017E	LATIN SMALL LETTER Z WITH CARON (ž)
                            0x9e => 0x017e,
                            // 0x9F	0x0178	LATIN CAPITAL LETTER Y WITH DIAERESIS (Ÿ)
                            0x9f => 0x0178,
                            _ => cr,
                        }
                    }
                    _ => value,
                };

                // Set the temporary buffer to the empty string.
                // Append a code point equal to the character reference code to the temporary
                // buffer.
                // Flush code points consumed as a character reference.
                // Switch to the return state.
                let old_temporary_buffer = self.temporary_buffer.clone();

                let mut raw =
                    String::with_capacity(old_temporary_buffer.len() + raw_char_ref.len() + 1);

                raw.push_str(&old_temporary_buffer);
                raw.push_str(&raw_char_ref);

                if self.cur == Some(';') {
                    raw.push(';');
                }

                self.temporary_buffer.clear();

                let c = match char::from_u32(cr) {
                    Some(c) => c,
                    _ => {
                        unreachable!();
                    }
                };

                self.temporary_buffer.push(c);
                self.flush_code_points_consumed_as_character_reference(Some(raw));
                self.state = self.return_state.clone();
            }
        }

        Ok(())
    }

    #[inline(always)]
    fn skip_next_lf(&mut self, c: char) {
        if c == '\r' && self.input.cur() == Some('\n') {
            self.input.bump();
        }
    }
}

// By spec '\r` removed before tokenizer, but we keep them to have better AST
// and don't break logic to ignore characters
#[inline(always)]
fn is_spacy(c: char) -> bool {
    matches!(c, '\x09' | '\x0a' | '\x0d' | '\x0c' | '\x20')
}

#[inline(always)]
fn is_control(c: u32) -> bool {
    matches!(c, c @ 0x00..=0x1f | c @ 0x7f..=0x9f if !matches!(c, 0x09 | 0x0a | 0x0c | 0x0d | 0x20))
}

#[inline(always)]
fn is_surrogate(c: u32) -> bool {
    matches!(c, 0xd800..=0xdfff)
}

// A noncharacter is a code point that is in the range U+FDD0 to U+FDEF,
// inclusive, or U+FFFE, U+FFFF, U+1FFFE, U+1FFFF, U+2FFFE, U+2FFFF, U+3FFFE,
// U+3FFFF, U+4FFFE, U+4FFFF, U+5FFFE, U+5FFFF, U+6FFFE, U+6FFFF, U+7FFFE,
// U+7FFFF, U+8FFFE, U+8FFFF, U+9FFFE, U+9FFFF, U+AFFFE, U+AFFFF, U+BFFFE,
// U+BFFFF, U+CFFFE, U+CFFFF, U+DFFFE, U+DFFFF, U+EFFFE, U+EFFFF, U+FFFFE,
// U+FFFFF, U+10FFFE, or U+10FFFF.
#[inline(always)]
fn is_noncharacter(c: u32) -> bool {
    matches!(
        c,
        0xfdd0
            ..=0xfdef
                | 0xfffe
                | 0xffff
                | 0x1fffe
                | 0x1ffff
                | 0x2fffe
                | 0x2ffff
                | 0x3fffe
                | 0x3ffff
                | 0x4fffe
                | 0x4ffff
                | 0x5fffe
                | 0x5ffff
                | 0x6fffe
                | 0x6ffff
                | 0x7fffe
                | 0x7ffff
                | 0x8fffe
                | 0x8ffff
                | 0x9fffe
                | 0x9ffff
                | 0xafffe
                | 0xaffff
                | 0xbfffe
                | 0xbffff
                | 0xcfffe
                | 0xcffff
                | 0xdfffe
                | 0xdffff
                | 0xefffe
                | 0xeffff
                | 0xffffe
                | 0xfffff
                | 0x10fffe
                | 0x10ffff,
    )
}

#[inline(always)]
fn is_upper_hex_digit(c: char) -> bool {
    matches!(c, '0'..='9' | 'A'..='F')
}

#[inline(always)]
fn is_lower_hex_digit(c: char) -> bool {
    matches!(c, '0'..='9' | 'a'..='f')
}

#[inline(always)]
fn is_ascii_hex_digit(c: char) -> bool {
    is_upper_hex_digit(c) || is_lower_hex_digit(c)
}

#[inline(always)]
fn is_ascii_upper_alpha(c: char) -> bool {
    matches!(c, 'A'..='Z')
}

#[inline(always)]
fn is_ascii_lower_alpha(c: char) -> bool {
    matches!(c, 'a'..='z')
}

#[inline(always)]
fn is_ascii_alpha(c: char) -> bool {
    is_ascii_upper_alpha(c) || is_ascii_lower_alpha(c)
}
