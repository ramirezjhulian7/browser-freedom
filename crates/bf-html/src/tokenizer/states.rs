//! Máquina de estados del tokenizador. Cada brazo del `match` transcribe la subsección
//! correspondiente de WHATWG HTML 13.2.5, en el mismo orden que el spec.

use super::{TagKind, Tokenizer};

/// Estados del tokenizador (13.2.5.1 a 13.2.5.76).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Data,
    Rcdata,
    Rawtext,
    ScriptData,
    Plaintext,
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
    ProcessingInstructionOpen,
    ProcessingInstructionTarget,
    AfterProcessingInstructionTarget,
    ProcessingInstructionData,
    ProcessingInstructionQuestionable,
    CharacterReference,
    NamedCharacterReference,
    AmbiguousAmpersand,
    NumericCharacterReference,
    HexadecimalCharacterReferenceStart,
    HexadecimalCharacterReference,
    DecimalCharacterReference,
    NumericCharacterReferenceEnd,
}

use State as S;

const REPLACEMENT: char = '\u{FFFD}';

/// TAB, LF, FF y SPACE: el conjunto que los estados del tokenizador tratan como espacio.
fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | ' ')
}

impl Tokenizer {
    /// Ejecuta un paso de la máquina de estados.
    pub(super) fn step(&mut self) {
        match self.state {
            // 13.2.5.1
            S::Data => match self.consume() {
                Some('&') => {
                    self.return_state = S::Data;
                    self.state = S::CharacterReference;
                }
                Some('<') => self.state = S::TagOpen,
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.emit_char('\0');
                }
                None => self.emit_eof(),
                Some(c) => self.emit_current(c),
            },
            // 13.2.5.2
            S::Rcdata => match self.consume() {
                Some('&') => {
                    self.return_state = S::Rcdata;
                    self.state = S::CharacterReference;
                }
                Some('<') => self.state = S::RcdataLessThanSign,
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.emit_char(REPLACEMENT);
                }
                None => self.emit_eof(),
                Some(c) => self.emit_current(c),
            },
            // 13.2.5.3
            S::Rawtext => match self.consume() {
                Some('<') => self.state = S::RawtextLessThanSign,
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.emit_char(REPLACEMENT);
                }
                None => self.emit_eof(),
                Some(c) => self.emit_current(c),
            },
            // 13.2.5.4
            S::ScriptData => match self.consume() {
                Some('<') => self.state = S::ScriptDataLessThanSign,
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.emit_char(REPLACEMENT);
                }
                None => self.emit_eof(),
                Some(c) => self.emit_current(c),
            },
            // 13.2.5.5
            S::Plaintext => match self.consume() {
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.emit_char(REPLACEMENT);
                }
                None => self.emit_eof(),
                Some(c) => self.emit_current(c),
            },
            // 13.2.5.6
            S::TagOpen => match self.consume() {
                Some('!') => self.state = S::MarkupDeclarationOpen,
                Some('/') => self.state = S::EndTagOpen,
                Some(c) if c.is_ascii_alphabetic() => {
                    self.create_tag(TagKind::Start);
                    self.reconsume_in(S::TagName);
                }
                Some('?') => {
                    self.temp.clear();
                    self.state = S::ProcessingInstructionOpen;
                }
                None => {
                    self.error("eof-before-tag-name");
                    self.emit_char('<');
                    self.emit_eof();
                }
                Some(_) => {
                    self.error("invalid-first-character-of-tag-name");
                    self.emit_char('<');
                    self.reconsume_in(S::Data);
                }
            },
            // 13.2.5.7
            S::EndTagOpen => match self.consume() {
                Some(c) if c.is_ascii_alphabetic() => {
                    self.create_tag(TagKind::End);
                    self.reconsume_in(S::TagName);
                }
                Some('>') => {
                    self.error("missing-end-tag-name");
                    self.state = S::Data;
                }
                None => {
                    self.error("eof-before-tag-name");
                    self.emit_char('<');
                    self.emit_char('/');
                    self.emit_eof();
                }
                Some(_) => {
                    self.error("invalid-first-character-of-tag-name");
                    self.comment.clear();
                    self.reconsume_in(S::BogusComment);
                }
            },
            // 13.2.5.8
            S::TagName => match self.consume() {
                Some(c) if is_space(c) => self.state = S::BeforeAttributeName,
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                Some(c) if c.is_ascii_uppercase() => self.tag.name.push(c.to_ascii_lowercase()),
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.tag.name.push(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-tag");
                    self.emit_eof();
                }
                Some(c) => self.tag.name.push(c),
            },
            // 13.2.5.9
            S::RcdataLessThanSign => self.text_less_than_sign(S::RcdataEndTagOpen, S::Rcdata),
            // 13.2.5.10
            S::RcdataEndTagOpen => self.text_end_tag_open(S::RcdataEndTagName, S::Rcdata),
            // 13.2.5.11
            S::RcdataEndTagName => self.text_end_tag_name(S::Rcdata),
            // 13.2.5.12
            S::RawtextLessThanSign => self.text_less_than_sign(S::RawtextEndTagOpen, S::Rawtext),
            // 13.2.5.13
            S::RawtextEndTagOpen => self.text_end_tag_open(S::RawtextEndTagName, S::Rawtext),
            // 13.2.5.14
            S::RawtextEndTagName => self.text_end_tag_name(S::Rawtext),
            // 13.2.5.15
            S::ScriptDataLessThanSign => match self.consume() {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataEndTagOpen;
                }
                Some('!') => {
                    self.state = S::ScriptDataEscapeStart;
                    self.emit_char('<');
                    self.emit_char('!');
                }
                _ => {
                    self.emit_char('<');
                    self.reconsume_in(S::ScriptData);
                }
            },
            // 13.2.5.16
            S::ScriptDataEndTagOpen => {
                self.text_end_tag_open(S::ScriptDataEndTagName, S::ScriptData);
            }
            // 13.2.5.17
            S::ScriptDataEndTagName => self.text_end_tag_name(S::ScriptData),
            // 13.2.5.18
            S::ScriptDataEscapeStart => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataEscapeStartDash;
                    self.emit_char('-');
                }
                _ => self.reconsume_in(S::ScriptData),
            },
            // 13.2.5.19
            S::ScriptDataEscapeStartDash => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                _ => self.reconsume_in(S::ScriptData),
            },
            // 13.2.5.20
            S::ScriptDataEscaped => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = S::ScriptDataEscapedLessThanSign,
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.emit_char(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-script-html-comment-like-text");
                    self.emit_eof();
                }
                Some(c) => self.emit_current(c),
            },
            // 13.2.5.21
            S::ScriptDataEscapedDash => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = S::ScriptDataEscapedLessThanSign,
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.state = S::ScriptDataEscaped;
                    self.emit_char(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-script-html-comment-like-text");
                    self.emit_eof();
                }
                Some(c) => {
                    self.state = S::ScriptDataEscaped;
                    self.emit_current(c);
                }
            },
            // 13.2.5.22
            S::ScriptDataEscapedDashDash => match self.consume() {
                Some('-') => self.emit_char('-'),
                Some('<') => self.state = S::ScriptDataEscapedLessThanSign,
                Some('>') => {
                    self.state = S::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.state = S::ScriptDataEscaped;
                    self.emit_char(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-script-html-comment-like-text");
                    self.emit_eof();
                }
                Some(c) => {
                    self.state = S::ScriptDataEscaped;
                    self.emit_current(c);
                }
            },
            // 13.2.5.23
            S::ScriptDataEscapedLessThanSign => match self.consume() {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataEscapedEndTagOpen;
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.temp.clear();
                    self.emit_char('<');
                    self.reconsume_in(S::ScriptDataDoubleEscapeStart);
                }
                _ => {
                    self.emit_char('<');
                    self.reconsume_in(S::ScriptDataEscaped);
                }
            },
            // 13.2.5.24
            S::ScriptDataEscapedEndTagOpen => {
                self.text_end_tag_open(S::ScriptDataEscapedEndTagName, S::ScriptDataEscaped);
            }
            // 13.2.5.25
            S::ScriptDataEscapedEndTagName => self.text_end_tag_name(S::ScriptDataEscaped),
            // 13.2.5.26
            S::ScriptDataDoubleEscapeStart => {
                self.double_escape_boundary(S::ScriptDataDoubleEscaped, S::ScriptDataEscaped);
            }
            // 13.2.5.27
            S::ScriptDataDoubleEscaped => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataDoubleEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.emit_char(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-script-html-comment-like-text");
                    self.emit_eof();
                }
                Some(c) => self.emit_current(c),
            },
            // 13.2.5.28
            S::ScriptDataDoubleEscapedDash => match self.consume() {
                Some('-') => {
                    self.state = S::ScriptDataDoubleEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-script-html-comment-like-text");
                    self.emit_eof();
                }
                Some(c) => {
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_current(c);
                }
            },
            // 13.2.5.29
            S::ScriptDataDoubleEscapedDashDash => match self.consume() {
                Some('-') => self.emit_char('-'),
                Some('<') => {
                    self.state = S::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('>') => {
                    self.state = S::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_char(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-script-html-comment-like-text");
                    self.emit_eof();
                }
                Some(c) => {
                    self.state = S::ScriptDataDoubleEscaped;
                    self.emit_current(c);
                }
            },
            // 13.2.5.30
            S::ScriptDataDoubleEscapedLessThanSign => match self.consume() {
                Some('/') => {
                    self.temp.clear();
                    self.state = S::ScriptDataDoubleEscapeEnd;
                    self.emit_char('/');
                }
                _ => self.reconsume_in(S::ScriptDataDoubleEscaped),
            },
            // 13.2.5.31
            S::ScriptDataDoubleEscapeEnd => {
                self.double_escape_boundary(S::ScriptDataEscaped, S::ScriptDataDoubleEscaped);
            }
            // 13.2.5.32
            S::BeforeAttributeName => match self.consume() {
                Some(c) if is_space(c) => {}
                Some('/' | '>') | None => self.reconsume_in(S::AfterAttributeName),
                Some('=') => {
                    self.error("unexpected-equals-sign-before-attribute-name");
                    self.start_attribute();
                    self.attr_name.push('=');
                    self.state = S::AttributeName;
                }
                Some(_) => {
                    self.start_attribute();
                    self.reconsume_in(S::AttributeName);
                }
            },
            // 13.2.5.33
            S::AttributeName => match self.consume() {
                Some(c) if is_space(c) => {
                    self.check_duplicate_attribute();
                    self.reconsume_in(S::AfterAttributeName);
                }
                Some('/' | '>') | None => {
                    self.check_duplicate_attribute();
                    self.reconsume_in(S::AfterAttributeName);
                }
                Some('=') => {
                    self.check_duplicate_attribute();
                    self.state = S::BeforeAttributeValue;
                }
                Some(c) if c.is_ascii_uppercase() => self.attr_name.push(c.to_ascii_lowercase()),
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.attr_name.push(REPLACEMENT);
                }
                Some(c) => {
                    if matches!(c, '"' | '\'' | '<') {
                        self.error("unexpected-character-in-attribute-name");
                    }
                    self.attr_name.push(c);
                }
            },
            // 13.2.5.34
            S::AfterAttributeName => match self.consume() {
                Some(c) if is_space(c) => {}
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('=') => self.state = S::BeforeAttributeValue,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => {
                    self.error("eof-in-tag");
                    self.emit_eof();
                }
                Some(_) => {
                    self.start_attribute();
                    self.reconsume_in(S::AttributeName);
                }
            },
            // 13.2.5.35
            S::BeforeAttributeValue => match self.consume() {
                Some(c) if is_space(c) => {}
                Some('"') => self.state = S::AttributeValueDoubleQuoted,
                Some('\'') => self.state = S::AttributeValueSingleQuoted,
                Some('>') => {
                    self.error("missing-attribute-value");
                    self.state = S::Data;
                    self.emit_tag();
                }
                _ => self.reconsume_in(S::AttributeValueUnquoted),
            },
            // 13.2.5.36
            S::AttributeValueDoubleQuoted => self.attribute_value_quoted('"'),
            // 13.2.5.37
            S::AttributeValueSingleQuoted => self.attribute_value_quoted('\''),
            // 13.2.5.38
            S::AttributeValueUnquoted => match self.consume() {
                Some(c) if is_space(c) => self.state = S::BeforeAttributeName,
                Some('&') => {
                    self.return_state = S::AttributeValueUnquoted;
                    self.state = S::CharacterReference;
                }
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.attr_value.push(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-tag");
                    self.emit_eof();
                }
                Some(c) => {
                    if matches!(c, '"' | '\'' | '<' | '=' | '`') {
                        self.error("unexpected-character-in-unquoted-attribute-value");
                    }
                    self.attr_value.push(c);
                }
            },
            // 13.2.5.39
            S::AfterAttributeValueQuoted => match self.consume() {
                Some(c) if is_space(c) => self.state = S::BeforeAttributeName,
                Some('/') => self.state = S::SelfClosingStartTag,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => {
                    self.error("eof-in-tag");
                    self.emit_eof();
                }
                Some(_) => {
                    self.error("missing-whitespace-between-attributes");
                    self.reconsume_in(S::BeforeAttributeName);
                }
            },
            // 13.2.5.40
            S::SelfClosingStartTag => match self.consume() {
                Some('>') => {
                    self.tag.self_closing = true;
                    self.state = S::Data;
                    self.emit_tag();
                }
                None => {
                    self.error("eof-in-tag");
                    self.emit_eof();
                }
                Some(_) => {
                    self.error("unexpected-solidus-in-tag");
                    self.reconsume_in(S::BeforeAttributeName);
                }
            },
            // 13.2.5.41
            S::BogusComment => match self.consume() {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.comment.push(REPLACEMENT);
                }
                Some(c) => self.comment.push(c),
            },
            // 13.2.5.42
            S::MarkupDeclarationOpen => {
                if self.next_chars_are("--", false) {
                    self.consume_n(2);
                    self.comment.clear();
                    self.state = S::CommentStart;
                } else if self.next_chars_are("DOCTYPE", true) {
                    self.consume_n(7);
                    self.state = S::Doctype;
                } else if self.next_chars_are("[CDATA[", false) {
                    self.consume_n(7);
                    if self.cdata_allowed {
                        self.state = S::CdataSection;
                    } else {
                        self.error("cdata-in-html-content");
                        self.comment.clear();
                        self.comment.push_str("[CDATA[");
                        self.state = S::BogusComment;
                    }
                } else {
                    self.error("incorrectly-opened-comment");
                    self.comment.clear();
                    self.state = S::BogusComment;
                }
            }
            // 13.2.5.43
            S::CommentStart => match self.consume() {
                Some('-') => self.state = S::CommentStartDash,
                Some('>') => {
                    self.error("abrupt-closing-of-empty-comment");
                    self.state = S::Data;
                    self.emit_comment();
                }
                _ => self.reconsume_in(S::Comment),
            },
            // 13.2.5.44
            S::CommentStartDash => match self.consume() {
                Some('-') => self.state = S::CommentEnd,
                Some('>') => {
                    self.error("abrupt-closing-of-empty-comment");
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.error("eof-in-comment");
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push('-');
                    self.reconsume_in(S::Comment);
                }
            },
            // 13.2.5.45
            S::Comment => match self.consume() {
                Some('<') => {
                    self.comment.push('<');
                    self.state = S::CommentLessThanSign;
                }
                Some('-') => self.state = S::CommentEndDash,
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.comment.push(REPLACEMENT);
                }
                None => {
                    self.error("eof-in-comment");
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(c) => self.comment.push(c),
            },
            // 13.2.5.46
            S::CommentLessThanSign => match self.consume() {
                Some('!') => {
                    self.comment.push('!');
                    self.state = S::CommentLessThanSignBang;
                }
                Some('<') => self.comment.push('<'),
                _ => self.reconsume_in(S::Comment),
            },
            // 13.2.5.47
            S::CommentLessThanSignBang => match self.consume() {
                Some('-') => self.state = S::CommentLessThanSignBangDash,
                _ => self.reconsume_in(S::Comment),
            },
            // 13.2.5.48
            S::CommentLessThanSignBangDash => match self.consume() {
                Some('-') => self.state = S::CommentLessThanSignBangDashDash,
                _ => self.reconsume_in(S::CommentEndDash),
            },
            // 13.2.5.49
            S::CommentLessThanSignBangDashDash => match self.consume() {
                Some('>') | None => self.reconsume_in(S::CommentEnd),
                Some(_) => {
                    self.error("nested-comment");
                    self.reconsume_in(S::CommentEnd);
                }
            },
            // 13.2.5.50
            S::CommentEndDash => match self.consume() {
                Some('-') => self.state = S::CommentEnd,
                None => {
                    self.error("eof-in-comment");
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push('-');
                    self.reconsume_in(S::Comment);
                }
            },
            // 13.2.5.51
            S::CommentEnd => match self.consume() {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_comment();
                }
                Some('!') => self.state = S::CommentEndBang,
                Some('-') => self.comment.push('-'),
                None => {
                    self.error("eof-in-comment");
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push_str("--");
                    self.reconsume_in(S::Comment);
                }
            },
            // 13.2.5.52
            S::CommentEndBang => match self.consume() {
                Some('-') => {
                    self.comment.push_str("--!");
                    self.state = S::CommentEndDash;
                }
                Some('>') => {
                    self.error("incorrectly-closed-comment");
                    self.state = S::Data;
                    self.emit_comment();
                }
                None => {
                    self.error("eof-in-comment");
                    self.emit_comment();
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.push_str("--!");
                    self.reconsume_in(S::Comment);
                }
            },
            // 13.2.5.53
            S::Doctype => match self.consume() {
                Some(c) if is_space(c) => self.state = S::BeforeDoctypeName,
                Some('>') => self.reconsume_in(S::BeforeDoctypeName),
                None => {
                    self.error("eof-in-doctype");
                    self.create_doctype();
                    self.doctype.force_quirks = true;
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(_) => {
                    self.error("missing-whitespace-before-doctype-name");
                    self.reconsume_in(S::BeforeDoctypeName);
                }
            },
            // 13.2.5.54
            S::BeforeDoctypeName => match self.consume() {
                Some(c) if is_space(c) => {}
                Some(c) if c.is_ascii_uppercase() => {
                    self.create_doctype();
                    self.doctype.name = Some(c.to_ascii_lowercase().to_string());
                    self.state = S::DoctypeName;
                }
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.create_doctype();
                    self.doctype.name = Some(REPLACEMENT.to_string());
                    self.state = S::DoctypeName;
                }
                Some('>') => {
                    self.error("missing-doctype-name");
                    self.create_doctype();
                    self.doctype.force_quirks = true;
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => {
                    self.error("eof-in-doctype");
                    self.create_doctype();
                    self.doctype.force_quirks = true;
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(c) => {
                    self.create_doctype();
                    self.doctype.name = Some(c.to_string());
                    self.state = S::DoctypeName;
                }
            },
            // 13.2.5.55
            S::DoctypeName => match self.consume() {
                Some(c) if is_space(c) => self.state = S::AfterDoctypeName,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                Some(c) if c.is_ascii_uppercase() => {
                    self.doctype_name().push(c.to_ascii_lowercase());
                }
                Some('\0') => {
                    self.error("unexpected-null-character");
                    self.doctype_name().push(REPLACEMENT);
                }
                None => self.eof_in_doctype(),
                Some(c) => self.doctype_name().push(c),
            },
            // 13.2.5.56
            S::AfterDoctypeName => match self.consume() {
                Some(c) if is_space(c) => {}
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    // "The six characters starting from the current input character".
                    let start = self.pos - 1;
                    if self.chars_match_at(start, "PUBLIC", true) {
                        self.consume_n(5);
                        self.state = S::AfterDoctypePublicKeyword;
                    } else if self.chars_match_at(start, "SYSTEM", true) {
                        self.consume_n(5);
                        self.state = S::AfterDoctypeSystemKeyword;
                    } else {
                        self.error("invalid-character-sequence-after-doctype-name");
                        self.doctype.force_quirks = true;
                        self.reconsume_in(S::BogusDoctype);
                    }
                }
            },
            // 13.2.5.57
            S::AfterDoctypePublicKeyword => match self.consume() {
                Some(c) if is_space(c) => self.state = S::BeforeDoctypePublicIdentifier,
                Some(q @ ('"' | '\'')) => {
                    self.error("missing-whitespace-after-doctype-public-keyword");
                    self.doctype.public_id = Some(String::new());
                    self.state = public_identifier_state(q);
                }
                Some('>') => {
                    self.error("missing-doctype-public-identifier");
                    self.doctype.force_quirks = true;
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    self.error("missing-quote-before-doctype-public-identifier");
                    self.doctype.force_quirks = true;
                    self.reconsume_in(S::BogusDoctype);
                }
            },
            // 13.2.5.58
            S::BeforeDoctypePublicIdentifier => match self.consume() {
                Some(c) if is_space(c) => {}
                Some(q @ ('"' | '\'')) => {
                    self.doctype.public_id = Some(String::new());
                    self.state = public_identifier_state(q);
                }
                Some('>') => {
                    self.error("missing-doctype-public-identifier");
                    self.doctype.force_quirks = true;
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    self.error("missing-quote-before-doctype-public-identifier");
                    self.doctype.force_quirks = true;
                    self.reconsume_in(S::BogusDoctype);
                }
            },
            // 13.2.5.59
            S::DoctypePublicIdentifierDoubleQuoted => self.doctype_public_identifier('"'),
            // 13.2.5.60
            S::DoctypePublicIdentifierSingleQuoted => self.doctype_public_identifier('\''),
            // 13.2.5.61
            S::AfterDoctypePublicIdentifier => match self.consume() {
                Some(c) if is_space(c) => {
                    self.state = S::BetweenDoctypePublicAndSystemIdentifiers;
                }
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                Some(q @ ('"' | '\'')) => {
                    self.error("missing-whitespace-between-doctype-public-and-system-identifiers");
                    self.doctype.system_id = Some(String::new());
                    self.state = system_identifier_state(q);
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    self.error("missing-quote-before-doctype-system-identifier");
                    self.doctype.force_quirks = true;
                    self.reconsume_in(S::BogusDoctype);
                }
            },
            // 13.2.5.62
            S::BetweenDoctypePublicAndSystemIdentifiers => match self.consume() {
                Some(c) if is_space(c) => {}
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                Some(q @ ('"' | '\'')) => {
                    self.doctype.system_id = Some(String::new());
                    self.state = system_identifier_state(q);
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    self.error("missing-quote-before-doctype-system-identifier");
                    self.doctype.force_quirks = true;
                    self.reconsume_in(S::BogusDoctype);
                }
            },
            // 13.2.5.63
            S::AfterDoctypeSystemKeyword => match self.consume() {
                Some(c) if is_space(c) => self.state = S::BeforeDoctypeSystemIdentifier,
                Some(q @ ('"' | '\'')) => {
                    self.error("missing-whitespace-after-doctype-system-keyword");
                    self.doctype.system_id = Some(String::new());
                    self.state = system_identifier_state(q);
                }
                Some('>') => {
                    self.error("missing-doctype-system-identifier");
                    self.doctype.force_quirks = true;
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    self.error("missing-quote-before-doctype-system-identifier");
                    self.doctype.force_quirks = true;
                    self.reconsume_in(S::BogusDoctype);
                }
            },
            // 13.2.5.64
            S::BeforeDoctypeSystemIdentifier => match self.consume() {
                Some(c) if is_space(c) => {}
                Some(q @ ('"' | '\'')) => {
                    self.doctype.system_id = Some(String::new());
                    self.state = system_identifier_state(q);
                }
                Some('>') => {
                    self.error("missing-doctype-system-identifier");
                    self.doctype.force_quirks = true;
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    self.error("missing-quote-before-doctype-system-identifier");
                    self.doctype.force_quirks = true;
                    self.reconsume_in(S::BogusDoctype);
                }
            },
            // 13.2.5.65
            S::DoctypeSystemIdentifierDoubleQuoted => self.doctype_system_identifier('"'),
            // 13.2.5.66
            S::DoctypeSystemIdentifierSingleQuoted => self.doctype_system_identifier('\''),
            // 13.2.5.67
            S::AfterDoctypeSystemIdentifier => match self.consume() {
                Some(c) if is_space(c) => {}
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                None => self.eof_in_doctype(),
                Some(_) => {
                    // No activa el force-quirks flag.
                    self.error("unexpected-character-after-doctype-system-identifier");
                    self.reconsume_in(S::BogusDoctype);
                }
            },
            // 13.2.5.68
            S::BogusDoctype => match self.consume() {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_doctype();
                }
                Some('\0') => self.error("unexpected-null-character"),
                None => {
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(_) => {}
            },
            // 13.2.5.69. Los U+0000 se manejan en el tree builder (contenido foráneo).
            S::CdataSection => match self.consume() {
                Some(']') => self.state = S::CdataSectionBracket,
                None => {
                    self.error("eof-in-cdata");
                    self.emit_eof();
                }
                Some(c) => self.emit_current(c),
            },
            // 13.2.5.70
            S::CdataSectionBracket => match self.consume() {
                Some(']') => self.state = S::CdataSectionEnd,
                _ => {
                    self.emit_char(']');
                    self.reconsume_in(S::CdataSection);
                }
            },
            // 13.2.5.71
            S::CdataSectionEnd => match self.consume() {
                Some(']') => self.emit_char(']'),
                Some('>') => self.state = S::Data,
                _ => {
                    self.emit_char(']');
                    self.emit_char(']');
                    self.reconsume_in(S::CdataSection);
                }
            },
            // 13.2.5.72
            S::ProcessingInstructionOpen => match self.consume() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    self.reconsume_in(S::ProcessingInstructionTarget);
                }
                None => {
                    self.error("eof-in-processing-instruction");
                    self.emit_eof();
                }
                Some(_) => {
                    self.error("invalid-first-character-of-processing-instruction-target");
                    self.convert_temp_to_comment();
                    self.reconsume_in(S::BogusComment);
                }
            },
            // 13.2.5.73
            S::ProcessingInstructionTarget => match self.consume() {
                Some(c) if is_space(c) || c == '?' || c == '>' => {
                    let target = &self.temp;
                    if target.eq_ignore_ascii_case("xml")
                        || target.eq_ignore_ascii_case("xml-stylesheet")
                    {
                        self.error("disallowed-processing-instruction-target");
                        self.convert_temp_to_comment();
                        self.reconsume_in(S::BogusComment);
                    } else {
                        self.pi.target = self.temp.clone();
                        self.pi.data.clear();
                        self.reconsume_in(S::AfterProcessingInstructionTarget);
                    }
                }
                Some(c) if c.is_ascii_alphanumeric() || c == '-' || c == '_' => self.temp.push(c),
                None => {
                    self.error("eof-in-processing-instruction");
                    self.emit_eof();
                }
                Some(_) => {
                    self.error("invalid-processing-instruction-target");
                    self.convert_temp_to_comment();
                    self.reconsume_in(S::BogusComment);
                }
            },
            // 13.2.5.74
            S::AfterProcessingInstructionTarget => match self.consume() {
                Some(c) if is_space(c) => {}
                _ => self.reconsume_in(S::ProcessingInstructionData),
            },
            // 13.2.5.75
            S::ProcessingInstructionData => match self.consume() {
                Some('?') => self.state = S::ProcessingInstructionQuestionable,
                Some('>') => {
                    self.state = S::Data;
                    self.emit_processing_instruction();
                }
                None => {
                    self.error("eof-in-processing-instruction");
                    self.emit_eof();
                }
                Some(c) => self.pi.data.push(c),
            },
            // 13.2.5.76
            S::ProcessingInstructionQuestionable => match self.consume() {
                Some('>') => {
                    self.state = S::Data;
                    self.emit_processing_instruction();
                }
                None => {
                    self.error("eof-in-processing-instruction");
                    self.emit_eof();
                }
                Some(_) => {
                    self.pi.data.push('?');
                    self.reconsume_in(S::ProcessingInstructionData);
                }
            },
            // 13.2.5.77
            S::CharacterReference => {
                self.temp.clear();
                self.temp.push('&');
                match self.consume() {
                    Some(c) if c.is_ascii_alphanumeric() => {
                        self.reconsume_in(S::NamedCharacterReference);
                    }
                    Some('#') => {
                        self.temp.push('#');
                        self.state = S::NumericCharacterReference;
                    }
                    _ => {
                        self.flush_char_ref();
                        self.reconsume_in(self.return_state);
                    }
                }
            }
            // 13.2.5.78
            S::NamedCharacterReference => {
                // Se entra reconsumiendo: el primer carácter del identificador es el current
                // input character. Se rebobina para que la búsqueda lo incluya.
                if self.reconsume {
                    self.reconsume = false;
                    self.pos -= 1;
                }
                match super::match_named_entity(&self.input[self.pos..]) {
                    Some((len, value)) => {
                        for _ in 0..len {
                            if let Some(c) = self.consume() {
                                self.temp.push(c);
                            }
                        }
                        let last_is_semicolon = self.current == Some(';');
                        let next = self.input.get(self.pos).copied();
                        if self.char_ref_in_attribute()
                            && !last_is_semicolon
                            && next
                                .and_then(char::from_u32)
                                .is_some_and(|c| c == '=' || c.is_ascii_alphanumeric())
                        {
                            // Por razones históricas.
                            self.flush_char_ref();
                            self.state = self.return_state;
                        } else {
                            if !last_is_semicolon {
                                self.error("missing-semicolon-after-character-reference");
                            }
                            self.temp.clear();
                            self.temp.push_str(value);
                            self.flush_char_ref();
                            self.state = self.return_state;
                        }
                    }
                    None => {
                        self.flush_char_ref();
                        self.state = S::AmbiguousAmpersand;
                    }
                }
            }
            // 13.2.5.79
            S::AmbiguousAmpersand => match self.consume() {
                Some(c) if c.is_ascii_alphanumeric() => {
                    if self.char_ref_in_attribute() {
                        self.attr_value.push(c);
                    } else {
                        self.emit_current(c);
                    }
                }
                Some(';') => {
                    self.error("unknown-named-character-reference");
                    self.reconsume_in(self.return_state);
                }
                _ => self.reconsume_in(self.return_state),
            },
            // 13.2.5.80
            S::NumericCharacterReference => {
                self.char_ref_code = 0;
                match self.consume() {
                    Some(c @ ('x' | 'X')) => {
                        self.temp.push(c);
                        self.state = S::HexadecimalCharacterReferenceStart;
                    }
                    Some(c) if c.is_ascii_digit() => {
                        self.reconsume_in(S::DecimalCharacterReference);
                    }
                    _ => {
                        self.error("absence-of-digits-in-numeric-character-reference");
                        self.flush_char_ref();
                        self.reconsume_in(self.return_state);
                    }
                }
            }
            // 13.2.5.81
            S::HexadecimalCharacterReferenceStart => match self.consume() {
                Some(c) if c.is_ascii_hexdigit() => {
                    self.reconsume_in(S::HexadecimalCharacterReference);
                }
                _ => {
                    self.error("absence-of-digits-in-numeric-character-reference");
                    self.flush_char_ref();
                    self.reconsume_in(self.return_state);
                }
            },
            // 13.2.5.82
            S::HexadecimalCharacterReference => match self.consume() {
                Some(c) if c.is_ascii_hexdigit() => {
                    // `to_digit(16)` equivale a restar 0x30, 0x37 o 0x57 según el rango.
                    let digit = c.to_digit(16).unwrap_or_default();
                    self.add_char_ref_digit(16, digit);
                }
                Some(';') => self.state = S::NumericCharacterReferenceEnd,
                _ => {
                    self.error("missing-semicolon-after-character-reference");
                    self.reconsume_in(S::NumericCharacterReferenceEnd);
                }
            },
            // 13.2.5.83
            S::DecimalCharacterReference => match self.consume() {
                Some(c) if c.is_ascii_digit() => {
                    let digit = c.to_digit(10).unwrap_or_default();
                    self.add_char_ref_digit(10, digit);
                }
                Some(';') => self.state = S::NumericCharacterReferenceEnd,
                _ => {
                    self.error("missing-semicolon-after-character-reference");
                    self.reconsume_in(S::NumericCharacterReferenceEnd);
                }
            },
            // 13.2.5.84. No consume: si se llegó reconsumiendo, el flag `reconsume` sigue
            // activo y el return state reconsume el mismo carácter.
            S::NumericCharacterReferenceEnd => {
                let mut code = self.char_ref_code;
                if code == 0x00 {
                    self.error("null-character-reference");
                    code = 0xFFFD;
                }
                if code > 0x10FFFF {
                    self.error("character-reference-outside-unicode-range");
                    code = 0xFFFD;
                }
                if (0xD800..=0xDFFF).contains(&code) {
                    self.error("surrogate-character-reference");
                    code = 0xFFFD;
                }
                if super::is_noncharacter(code) {
                    self.error("noncharacter-character-reference");
                }
                let is_control = code <= 0x1F || (0x7F..=0x9F).contains(&code);
                let is_ascii_whitespace = matches!(code, 0x09 | 0x0A | 0x0C | 0x0D | 0x20);
                if code == 0x0D || (is_control && !is_ascii_whitespace) {
                    self.error("control-character-reference");
                    if let Some(replacement) = c1_replacement(code) {
                        code = replacement;
                    }
                }
                self.char_ref_code = code;
                self.temp.clear();
                // Los surrogates ya se reemplazaron por U+FFFD, así que `from_u32` no falla.
                self.temp.push(char::from_u32(code).unwrap_or(REPLACEMENT));
                self.flush_char_ref();
                self.state = self.return_state;
            }
        }
    }

    /// Multiplica el character reference code por `base` y le suma `digit`. Satura en vez de
    /// desbordar: cualquier valor por encima de 0x10FFFF ya produce el mismo resultado.
    fn add_char_ref_digit(&mut self, base: u32, digit: u32) {
        self.char_ref_code = self
            .char_ref_code
            .saturating_mul(base)
            .saturating_add(digit);
    }

    // --- Estados compartidos (texto idéntico en el spec salvo los estados destino) ---

    /// RCDATA / RAWTEXT less-than sign state (13.2.5.9, 13.2.5.12).
    fn text_less_than_sign(&mut self, end_tag_open: State, text: State) {
        match self.consume() {
            Some('/') => {
                self.temp.clear();
                self.state = end_tag_open;
            }
            _ => {
                self.emit_char('<');
                self.reconsume_in(text);
            }
        }
    }

    /// RCDATA / RAWTEXT / script data / script data escaped end tag open state
    /// (13.2.5.10, 13.2.5.13, 13.2.5.16, 13.2.5.24).
    fn text_end_tag_open(&mut self, end_tag_name: State, text: State) {
        match self.consume() {
            Some(c) if c.is_ascii_alphabetic() => {
                self.create_tag(TagKind::End);
                self.reconsume_in(end_tag_name);
            }
            _ => {
                self.emit_char('<');
                self.emit_char('/');
                self.reconsume_in(text);
            }
        }
    }

    /// RCDATA / RAWTEXT / script data / script data escaped end tag name state
    /// (13.2.5.11, 13.2.5.14, 13.2.5.17, 13.2.5.25).
    fn text_end_tag_name(&mut self, text: State) {
        match self.consume() {
            Some(c) if is_space(c) && self.is_appropriate_end_tag() => {
                self.state = S::BeforeAttributeName;
            }
            Some('/') if self.is_appropriate_end_tag() => self.state = S::SelfClosingStartTag,
            Some('>') if self.is_appropriate_end_tag() => {
                self.state = S::Data;
                self.emit_tag();
            }
            Some(c) if c.is_ascii_uppercase() => {
                self.tag.name.push(c.to_ascii_lowercase());
                self.temp.push(c);
            }
            Some(c) if c.is_ascii_lowercase() => {
                self.tag.name.push(c);
                self.temp.push(c);
            }
            _ => {
                self.emit_less_than_solidus_and_temp();
                self.reconsume_in(text);
            }
        }
    }

    /// Script data double escape start / end state (13.2.5.26, 13.2.5.31).
    fn double_escape_boundary(&mut self, if_script: State, otherwise: State) {
        match self.consume() {
            Some(c) if is_space(c) || c == '/' || c == '>' => {
                self.state = if self.temp == "script" {
                    if_script
                } else {
                    otherwise
                };
                self.emit_current(c);
            }
            Some(c) if c.is_ascii_uppercase() => {
                self.temp.push(c.to_ascii_lowercase());
                self.emit_current(c);
            }
            Some(c) if c.is_ascii_lowercase() => {
                self.temp.push(c);
                self.emit_current(c);
            }
            _ => self.reconsume_in(otherwise),
        }
    }

    /// Attribute value (double-quoted / single-quoted) state (13.2.5.36, 13.2.5.37).
    fn attribute_value_quoted(&mut self, quote: char) {
        match self.consume() {
            Some(c) if c == quote => self.state = S::AfterAttributeValueQuoted,
            Some('&') => {
                // El estado actual es el attribute value (double- o single-quoted) state.
                self.return_state = self.state;
                self.state = S::CharacterReference;
            }
            Some('\0') => {
                self.error("unexpected-null-character");
                self.attr_value.push(REPLACEMENT);
            }
            None => {
                self.error("eof-in-tag");
                self.emit_eof();
            }
            Some(c) => self.attr_value.push(c),
        }
    }

    /// DOCTYPE public identifier (double-quoted / single-quoted) state (13.2.5.59, 13.2.5.60).
    fn doctype_public_identifier(&mut self, quote: char) {
        match self.consume() {
            Some(c) if c == quote => self.state = S::AfterDoctypePublicIdentifier,
            Some('\0') => {
                self.error("unexpected-null-character");
                self.doctype_public_id().push(REPLACEMENT);
            }
            Some('>') => {
                self.error("abrupt-doctype-public-identifier");
                self.doctype.force_quirks = true;
                self.state = S::Data;
                self.emit_doctype();
            }
            None => self.eof_in_doctype(),
            Some(c) => self.doctype_public_id().push(c),
        }
    }

    /// DOCTYPE system identifier (double-quoted / single-quoted) state (13.2.5.65, 13.2.5.66).
    fn doctype_system_identifier(&mut self, quote: char) {
        match self.consume() {
            Some(c) if c == quote => self.state = S::AfterDoctypeSystemIdentifier,
            Some('\0') => {
                self.error("unexpected-null-character");
                self.doctype_system_id().push(REPLACEMENT);
            }
            Some('>') => {
                self.error("abrupt-doctype-system-identifier");
                self.doctype.force_quirks = true;
                self.state = S::Data;
                self.emit_doctype();
            }
            None => self.eof_in_doctype(),
            Some(c) => self.doctype_system_id().push(c),
        }
    }

    /// EOF dentro de un DOCTYPE ya creado: error, force-quirks, emitir DOCTYPE y EOF.
    fn eof_in_doctype(&mut self) {
        self.error("eof-in-doctype");
        self.doctype.force_quirks = true;
        self.emit_doctype();
        self.emit_eof();
    }

    fn doctype_name(&mut self) -> &mut String {
        self.doctype.name.get_or_insert_default()
    }

    fn doctype_public_id(&mut self) -> &mut String {
        self.doctype.public_id.get_or_insert_default()
    }

    fn doctype_system_id(&mut self) -> &mut String {
        self.doctype.system_id.get_or_insert_default()
    }
}

/// Tabla de 13.2.5.84 (numeric character reference end state): code points de C1 que se
/// reinterpretan como windows-1252.
fn c1_replacement(code: u32) -> Option<u32> {
    Some(match code {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        _ => return None,
    })
}

fn public_identifier_state(quote: char) -> State {
    if quote == '"' {
        S::DoctypePublicIdentifierDoubleQuoted
    } else {
        S::DoctypePublicIdentifierSingleQuoted
    }
}

fn system_identifier_state(quote: char) -> State {
    if quote == '"' {
        S::DoctypeSystemIdentifierDoubleQuoted
    } else {
        S::DoctypeSystemIdentifierSingleQuoted
    }
}
