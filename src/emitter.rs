//! Custom emitter with typed event stream support.
//!
//! Provides [`Emitter`] which wraps libfyaml's emitter and returns a typed
//! stream of [`EmitEvent`]s during emission. This allows callers to inspect
//! and post-process output with full knowledge of the write type (indent,
//! key, scalar, linebreak, etc.).

use crate::config;
use crate::document::Document;
use crate::error::{Error, Result};
use fyaml_sys::*;
use std::fmt;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

/// The type of content being written by the emitter.
///
/// Maps libfyaml's `fy_emitter_write_type` to Rust. Unknown values from
/// future libfyaml versions are captured by [`Other`](WriteType::Other).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteType {
    // -- document structure --
    /// A document indicator (`---` or `...`).
    DocumentIndicator,
    /// A `%TAG` directive.
    TagDirective,
    /// A `%YAML` version directive.
    VersionDirective,

    // -- whitespace / formatting --
    /// Indentation whitespace.
    Indent,
    /// Non-indent whitespace (e.g. space between key and value).
    Whitespace,
    /// A newline.
    Linebreak,
    /// A terminating null byte.
    TerminatingZero,

    // -- generic indicator (non-extended mode) --
    /// A generic indicator character (used when extended indicators are off).
    Indicator,

    // -- scalars --
    /// A plain (unquoted) scalar value.
    PlainScalar,
    /// A single-quoted scalar value.
    SingleQuotedScalar,
    /// A double-quoted scalar value.
    DoubleQuotedScalar,
    /// Content of a literal block scalar (`|`).
    LiteralScalar,
    /// Content of a folded block scalar (`>`).
    FoldedScalar,

    // -- scalar keys --
    /// A plain scalar used as a mapping key.
    PlainScalarKey,
    /// A single-quoted scalar used as a mapping key.
    SingleQuotedScalarKey,
    /// A double-quoted scalar used as a mapping key.
    DoubleQuotedScalarKey,

    // -- anchors, aliases, tags --
    /// An anchor (`&name`).
    Anchor,
    /// A tag (e.g. `!!str`).
    Tag,
    /// An alias (`*name`).
    Alias,

    // -- comments --
    /// A comment.
    Comment,

    // -- extended indicators --
    /// The `?` indicator (explicit mapping key).
    IndicatorQuestionMark,
    /// The `:` indicator (mapping value).
    IndicatorColon,
    /// The `-` indicator (sequence item).
    IndicatorDash,
    /// The `[` indicator (flow sequence start).
    IndicatorLeftBracket,
    /// The `]` indicator (flow sequence end).
    IndicatorRightBracket,
    /// The `{` indicator (flow mapping start).
    IndicatorLeftBrace,
    /// The `}` indicator (flow mapping end).
    IndicatorRightBrace,
    /// The `,` indicator (flow separator).
    IndicatorComma,
    /// The `|` indicator (literal block scalar header).
    IndicatorBar,
    /// The `>` indicator (folded block scalar header).
    IndicatorGreater,
    /// The opening `'` of a single-quoted scalar.
    IndicatorSingleQuoteStart,
    /// The closing `'` of a single-quoted scalar.
    IndicatorSingleQuoteEnd,
    /// The opening `"` of a double-quoted scalar.
    IndicatorDoubleQuoteStart,
    /// The closing `"` of a double-quoted scalar.
    IndicatorDoubleQuoteEnd,
    /// The `&` indicator (anchor prefix).
    IndicatorAmpersand,
    /// The `*` indicator (alias prefix).
    IndicatorStar,
    /// A chomp indicator (`-` or `+` on block scalars).
    IndicatorChomp,
    /// An explicit indent indicator (`0`–`9` on block scalars).
    IndicatorExplicitIndent,

    /// Any write type not recognised by this version of the crate.
    Other(u32),
}

impl fmt::Display for WriteType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteType::DocumentIndicator => f.write_str("document indicator"),
            WriteType::TagDirective => f.write_str("tag directive"),
            WriteType::VersionDirective => f.write_str("version directive"),
            WriteType::Indent => f.write_str("indent"),
            WriteType::Whitespace => f.write_str("whitespace"),
            WriteType::Linebreak => f.write_str("linebreak"),
            WriteType::TerminatingZero => f.write_str("terminating zero"),
            WriteType::Indicator => f.write_str("indicator"),
            WriteType::PlainScalar => f.write_str("plain scalar"),
            WriteType::SingleQuotedScalar => f.write_str("single-quoted scalar"),
            WriteType::DoubleQuotedScalar => f.write_str("double-quoted scalar"),
            WriteType::LiteralScalar => f.write_str("literal scalar"),
            WriteType::FoldedScalar => f.write_str("folded scalar"),
            WriteType::PlainScalarKey => f.write_str("plain scalar key"),
            WriteType::SingleQuotedScalarKey => f.write_str("single-quoted scalar key"),
            WriteType::DoubleQuotedScalarKey => f.write_str("double-quoted scalar key"),
            WriteType::Anchor => f.write_str("anchor"),
            WriteType::Tag => f.write_str("tag"),
            WriteType::Alias => f.write_str("alias"),
            WriteType::Comment => f.write_str("comment"),
            WriteType::IndicatorQuestionMark => f.write_str("'?' indicator"),
            WriteType::IndicatorColon => f.write_str("':' indicator"),
            WriteType::IndicatorDash => f.write_str("'-' indicator"),
            WriteType::IndicatorLeftBracket => f.write_str("'[' indicator"),
            WriteType::IndicatorRightBracket => f.write_str("']' indicator"),
            WriteType::IndicatorLeftBrace => f.write_str("'{' indicator"),
            WriteType::IndicatorRightBrace => f.write_str("'}' indicator"),
            WriteType::IndicatorComma => f.write_str("',' indicator"),
            WriteType::IndicatorBar => f.write_str("'|' indicator"),
            WriteType::IndicatorGreater => f.write_str("'>' indicator"),
            WriteType::IndicatorSingleQuoteStart => f.write_str("opening '\\'' indicator"),
            WriteType::IndicatorSingleQuoteEnd => f.write_str("closing '\\'' indicator"),
            WriteType::IndicatorDoubleQuoteStart => f.write_str("opening '\"' indicator"),
            WriteType::IndicatorDoubleQuoteEnd => f.write_str("closing '\"' indicator"),
            WriteType::IndicatorAmpersand => f.write_str("'&' indicator"),
            WriteType::IndicatorStar => f.write_str("'*' indicator"),
            WriteType::IndicatorChomp => f.write_str("chomp indicator"),
            WriteType::IndicatorExplicitIndent => f.write_str("explicit indent indicator"),
            WriteType::Other(v) => write!(f, "unknown write type ({v})"),
        }
    }
}

impl From<u32> for WriteType {
    fn from(raw: u32) -> Self {
        match raw {
            x if x == fyewt_document_indicator => WriteType::DocumentIndicator,
            x if x == fyewt_tag_directive => WriteType::TagDirective,
            x if x == fyewt_version_directive => WriteType::VersionDirective,
            x if x == fyewt_indent => WriteType::Indent,
            x if x == fyewt_indicator => WriteType::Indicator,
            x if x == fyewt_whitespace => WriteType::Whitespace,
            x if x == fyewt_plain_scalar => WriteType::PlainScalar,
            x if x == fyewt_single_quoted_scalar => WriteType::SingleQuotedScalar,
            x if x == fyewt_double_quoted_scalar => WriteType::DoubleQuotedScalar,
            x if x == fyewt_literal_scalar => WriteType::LiteralScalar,
            x if x == fyewt_folded_scalar => WriteType::FoldedScalar,
            x if x == fyewt_anchor => WriteType::Anchor,
            x if x == fyewt_tag => WriteType::Tag,
            x if x == fyewt_linebreak => WriteType::Linebreak,
            x if x == fyewt_alias => WriteType::Alias,
            x if x == fyewt_terminating_zero => WriteType::TerminatingZero,
            x if x == fyewt_plain_scalar_key => WriteType::PlainScalarKey,
            x if x == fyewt_single_quoted_scalar_key => WriteType::SingleQuotedScalarKey,
            x if x == fyewt_double_quoted_scalar_key => WriteType::DoubleQuotedScalarKey,
            x if x == fyewt_comment => WriteType::Comment,
            x if x == fyewt_indicator_question_mark => WriteType::IndicatorQuestionMark,
            x if x == fyewt_indicator_colon => WriteType::IndicatorColon,
            x if x == fyewt_indicator_dash => WriteType::IndicatorDash,
            x if x == fyewt_indicator_left_bracket => WriteType::IndicatorLeftBracket,
            x if x == fyewt_indicator_right_bracket => WriteType::IndicatorRightBracket,
            x if x == fyewt_indicator_left_brace => WriteType::IndicatorLeftBrace,
            x if x == fyewt_indicator_right_brace => WriteType::IndicatorRightBrace,
            x if x == fyewt_indicator_comma => WriteType::IndicatorComma,
            x if x == fyewt_indicator_bar => WriteType::IndicatorBar,
            x if x == fyewt_indicator_greater => WriteType::IndicatorGreater,
            x if x == fyewt_indicator_single_quote_start => WriteType::IndicatorSingleQuoteStart,
            x if x == fyewt_indicator_single_quote_end => WriteType::IndicatorSingleQuoteEnd,
            x if x == fyewt_indicator_double_quote_start => WriteType::IndicatorDoubleQuoteStart,
            x if x == fyewt_indicator_double_quote_end => WriteType::IndicatorDoubleQuoteEnd,
            x if x == fyewt_indicator_ambersand => WriteType::IndicatorAmpersand,
            x if x == fyewt_indicator_star => WriteType::IndicatorStar,
            x if x == fyewt_indicator_chomp => WriteType::IndicatorChomp,
            x if x == fyewt_indicator_explicit_indent => WriteType::IndicatorExplicitIndent,
            other => WriteType::Other(other),
        }
    }
}

/// A single emission event produced by [`Emitter::emit_events`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitEvent {
    pub write_type: WriteType,
    pub content: String,
}

/// Output mode for the emitter.
///
/// Controls how the YAML is formatted: block style, flow style, JSON, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitMode {
    /// Preserve the original formatting style.
    Original,
    /// Force block style output.
    Block,
    /// Force flow style output.
    Flow,
    /// Flow style on a single line.
    FlowOneline,
    /// JSON output (non type-preserving).
    Json,
    /// JSON output (type-preserving).
    JsonTyped,
    /// JSON output on a single line.
    JsonOneline,
    /// Pretty-print JSON as YAML.
    Dejson,
    /// Pretty YAML output.
    Pretty,
    /// Respect manual style hints on nodes.
    Manual,
    /// Flow style, compact.
    FlowCompact,
    /// JSON output, compact.
    JsonCompact,
}

/// Tri-state toggle for optional document markers and directives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// Emit automatically when required.
    Auto,
    /// Never emit.
    Off,
    /// Always emit.
    On,
}

// ---------------------------------------------------------------------------
// Bitfield helpers (private)
// ---------------------------------------------------------------------------

const INDENT_SHIFT: u32 = 8;
const INDENT_MASK: u32 = 0xf;
const WIDTH_SHIFT: u32 = 12;
const WIDTH_MASK: u32 = 0xff;
const MODE_SHIFT: u32 = 20;
const MODE_MASK: u32 = 0xf;
const DOC_START_MARK_SHIFT: u32 = 24;
const DOC_START_MARK_MASK: u32 = 0x3;
const DOC_END_MARK_SHIFT: u32 = 26;
const DOC_END_MARK_MASK: u32 = 0x3;
const VERSION_DIR_SHIFT: u32 = 28;
const VERSION_DIR_MASK: u32 = 0x3;
const TAG_DIR_SHIFT: u32 = 30;
const TAG_DIR_MASK: u32 = 0x3;

/// Set a multi-bit field within the flags word.
#[inline]
fn set_field(flags: u32, value: u32, mask: u32, shift: u32) -> u32 {
    (flags & !(mask << shift)) | ((value & mask) << shift)
}

/// Set or clear a single bit.
#[inline]
fn set_bit(flags: u32, bit: u32, on: bool) -> u32 {
    if on {
        flags | bit
    } else {
        flags & !bit
    }
}

/// A configurable emitter handle tied to a [`Document`].
///
/// Created via [`Document::emitter()`]. Builder methods configure output
/// formatting; [`emit_events`](Self::emit_events) performs the emission.
///
/// By default the emitter preserves original formatting and outputs comments,
/// matching the behaviour of [`Document::emit()`].
///
/// # Example
///
/// ```
/// use fyaml::{Document, EmitMode};
///
/// let doc = Document::parse_str("foo: bar\nbaz: qux").unwrap();
/// let events = doc.emitter()
///     .mode(EmitMode::Json)
///     .emit_events()
///     .unwrap();
/// let json: String = events.iter().map(|e| e.content.as_str()).collect();
/// assert!(json.contains('{'));
/// ```
pub struct Emitter<'doc> {
    doc: &'doc Document,
    flags: u32,
    xflags: u32,
}

impl<'doc> Emitter<'doc> {
    /// Creates a new emitter for the document with default flags.
    #[inline]
    pub(crate) fn new(doc: &'doc Document) -> Self {
        Emitter {
            doc,
            flags: config::emit_flags(),
            xflags: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Multi-value builders
    // -----------------------------------------------------------------------

    /// Sets the output mode.
    #[inline]
    pub fn mode(mut self, mode: EmitMode) -> Self {
        let v = match mode {
            EmitMode::Original => 0,
            EmitMode::Block => 1,
            EmitMode::Flow => 2,
            EmitMode::FlowOneline => 3,
            EmitMode::Json => 4,
            EmitMode::JsonTyped => 5,
            EmitMode::JsonOneline => 6,
            EmitMode::Dejson => 7,
            EmitMode::Pretty => 8,
            EmitMode::Manual => 9,
            EmitMode::FlowCompact => 10,
            EmitMode::JsonCompact => 11,
        };
        self.flags = set_field(self.flags, v, MODE_MASK, MODE_SHIFT);
        self
    }

    /// Sets the indentation level (0–9). Values above 9 are clamped.
    #[inline]
    pub fn indent(mut self, n: u8) -> Self {
        let v = u32::from(n.min(9));
        self.flags = set_field(self.flags, v, INDENT_MASK, INDENT_SHIFT);
        self
    }

    /// Sets the line width (0–254). Use [`width_infinite`](Self::width_infinite) for no limit.
    #[inline]
    pub fn width(mut self, w: u8) -> Self {
        let v = u32::from(w);
        self.flags = set_field(self.flags, v, WIDTH_MASK, WIDTH_SHIFT);
        self
    }

    /// Disables line-width wrapping (width = 255 / infinite).
    #[inline]
    pub fn width_infinite(mut self) -> Self {
        self.flags = set_field(self.flags, 255, WIDTH_MASK, WIDTH_SHIFT);
        self
    }

    // -----------------------------------------------------------------------
    // Boolean builders
    // -----------------------------------------------------------------------

    /// Sort mapping keys alphabetically when emitting.
    #[inline]
    pub fn sort_keys(mut self, on: bool) -> Self {
        self.flags = set_bit(self.flags, FYECF_SORT_KEYS, on);
        self
    }

    /// Include comments in the output.
    #[inline]
    pub fn output_comments(mut self, on: bool) -> Self {
        self.flags = set_bit(self.flags, FYECF_OUTPUT_COMMENTS, on);
        self
    }

    /// Strip anchor/alias labels from the output.
    #[inline]
    pub fn strip_labels(mut self, on: bool) -> Self {
        self.flags = set_bit(self.flags, FYECF_STRIP_LABELS, on);
        self
    }

    /// Strip tags from the output.
    #[inline]
    pub fn strip_tags(mut self, on: bool) -> Self {
        self.flags = set_bit(self.flags, FYECF_STRIP_TAGS, on);
        self
    }

    /// Strip document indicators and directives.
    #[inline]
    pub fn strip_doc(mut self, on: bool) -> Self {
        self.flags = set_bit(self.flags, FYECF_STRIP_DOC, on);
        self
    }

    /// Suppress the trailing newline at the end of output.
    #[inline]
    pub fn no_ending_newline(mut self, on: bool) -> Self {
        self.flags = set_bit(self.flags, FYECF_NO_ENDING_NEWLINE, on);
        self
    }

    /// Strip mapping entries whose value is empty.
    #[inline]
    pub fn strip_empty_kv(mut self, on: bool) -> Self {
        self.flags = set_bit(self.flags, FYECF_STRIP_EMPTY_KV, on);
        self
    }

    // -----------------------------------------------------------------------
    // Extended configuration builders
    // -----------------------------------------------------------------------

    /// Indent block sequences that are mapping values.
    ///
    /// When enabled, block sequences that appear as mapping values are indented
    /// relative to their key, matching the common GitHub Actions / YAML style:
    ///
    /// ```yaml
    /// steps:
    ///   - name: foo    # indented (on)
    /// ```
    ///
    /// vs the default:
    ///
    /// ```yaml
    /// steps:
    /// - name: foo      # not indented (off)
    /// ```
    #[inline]
    pub fn indented_seq_in_map(mut self, on: bool) -> Self {
        self.xflags = set_bit(self.xflags, FYEXCF_INDENTED_SEQ_IN_MAP, on);
        self
    }

    /// Preserve oneline flow collections during emission.
    ///
    /// When enabled, flow collections (`[a, b]`, `{k: v}`) that were written
    /// on a single line in the source are kept on one line in the output,
    /// rather than being reflowed to multiple lines.
    #[inline]
    pub fn preserve_flow_layout(mut self, on: bool) -> Self {
        self.xflags = set_bit(self.xflags, FYEXCF_PRESERVE_FLOW_LAYOUT, on);
        self
    }

    // -----------------------------------------------------------------------
    // Tri-state (Toggle) builders
    // -----------------------------------------------------------------------

    /// Controls emission of the document start marker (`---`).
    #[inline]
    pub fn doc_start_mark(mut self, toggle: Toggle) -> Self {
        self.flags = set_field(
            self.flags,
            toggle as u32,
            DOC_START_MARK_MASK,
            DOC_START_MARK_SHIFT,
        );
        self
    }

    /// Controls emission of the document end marker (`...`).
    #[inline]
    pub fn doc_end_mark(mut self, toggle: Toggle) -> Self {
        self.flags = set_field(
            self.flags,
            toggle as u32,
            DOC_END_MARK_MASK,
            DOC_END_MARK_SHIFT,
        );
        self
    }

    /// Controls emission of the `%YAML` version directive.
    #[inline]
    pub fn version_directive(mut self, toggle: Toggle) -> Self {
        self.flags = set_field(
            self.flags,
            toggle as u32,
            VERSION_DIR_MASK,
            VERSION_DIR_SHIFT,
        );
        self
    }

    /// Controls emission of `%TAG` directives.
    #[inline]
    pub fn tag_directive(mut self, toggle: Toggle) -> Self {
        self.flags = set_field(self.flags, toggle as u32, TAG_DIR_MASK, TAG_DIR_SHIFT);
        self
    }

    /// Emits the document as a stream of typed [`EmitEvent`]s.
    ///
    /// Each event carries its [`WriteType`] and content string, allowing
    /// callers to post-process the output with full structural context.
    ///
    /// # Example
    ///
    /// ```
    /// use fyaml::{Document, WriteType};
    ///
    /// let doc = Document::parse_str("foo: bar\nbaz: qux").unwrap();
    /// let events = doc.emitter().emit_events().unwrap();
    /// let yaml: String = events.iter().map(|e| e.content.as_str()).collect();
    /// assert!(yaml.contains("foo: bar"));
    /// ```
    pub fn emit_events(&self) -> Result<Vec<EmitEvent>> {
        let mut state = CallbackState { events: Vec::new() };

        let output = Some(trampoline as _);
        let userdata = &mut state as *mut CallbackState as *mut c_void;

        let emitter = if self.xflags != 0 {
            let xcfg = fy_emitter_xcfg {
                cfg: fy_emitter_cfg {
                    flags: self.flags | FYECF_EXTENDED_CFG,
                    output,
                    userdata,
                    diag: ptr::null_mut(),
                },
                xflags: self.xflags,
                colors: [ptr::null(); 38],
                __bindgen_anon_1: unsafe { std::mem::zeroed() },
            };
            // SAFETY: When FYECF_EXTENDED_CFG is set, fy_emitter_create reads
            // the surrounding fy_emitter_xcfg via container_of on the cfg pointer.
            // The xcfg lives on the stack and is valid for the duration of the call.
            unsafe { fy_emitter_create(&xcfg.cfg) }
        } else {
            let cfg = fy_emitter_cfg {
                flags: self.flags,
                output,
                userdata,
                diag: ptr::null_mut(),
            };
            // SAFETY: fy_emitter_create takes a pointer to the cfg and copies it.
            // The emitter is destroyed before `state` goes out of scope.
            unsafe { fy_emitter_create(&cfg) }
        };

        if emitter.is_null() {
            return Err(Error::Ffi("fy_emitter_create returned null"));
        }

        let rc = unsafe { fy_emit_document(emitter, self.doc.as_ptr()) };
        unsafe { fy_emitter_destroy(emitter) };

        if rc != 0 {
            return Err(Error::Ffi("fy_emit_document failed"));
        }

        Ok(state.events)
    }
}

/// Internal state passed through the C callback's userdata pointer.
struct CallbackState {
    events: Vec<EmitEvent>,
}

/// C-compatible trampoline that bridges libfyaml's output callback to event collection.
///
/// # Safety
///
/// - `userdata` must point to a valid `CallbackState`
/// - `str_` must be a valid pointer to `len` bytes of UTF-8 data
unsafe extern "C" fn trampoline(
    _emit: *mut fy_emitter,
    write_type: fy_emitter_write_type,
    str_: *const c_char,
    len: c_int,
    userdata: *mut c_void,
) -> c_int {
    let state = &mut *(userdata as *mut CallbackState);
    let content =
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(str_ as *const u8, len as usize));

    state.events.push(EmitEvent {
        write_type: WriteType::from(write_type),
        content: content.to_owned(),
    });

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events_to_string(events: &[EmitEvent]) -> String {
        events.iter().map(|e| e.content.as_str()).collect()
    }

    #[test]
    fn emit_events_identity() {
        let doc = Document::parse_str("foo: bar").unwrap();
        let normal = doc.emit().unwrap();
        let events = doc.emitter().emit_events().unwrap();
        assert_eq!(normal, events_to_string(&events));
    }

    #[test]
    fn emit_events_receives_write_types() {
        let doc = Document::parse_str("key: value").unwrap();
        let events = doc.emitter().emit_events().unwrap();
        let types: Vec<_> = events.iter().map(|e| e.write_type).collect();
        assert!(types.contains(&WriteType::PlainScalarKey));
        assert!(types.contains(&WriteType::Linebreak));
    }

    #[test]
    fn emit_events_handles_block_scalar() {
        let doc = Document::parse_str("run: |\n  echo hello\n  echo world").unwrap();
        let events = doc.emitter().emit_events().unwrap();
        assert!(events
            .iter()
            .any(|e| e.write_type == WriteType::LiteralScalar));
    }

    #[test]
    fn emit_events_preserves_comments() {
        let doc = Document::parse_str("# top comment\nfoo: bar # inline").unwrap();
        let events = doc.emitter().emit_events().unwrap();
        assert!(events.iter().any(|e| e.write_type == WriteType::Comment));
        let text = events_to_string(&events);
        assert!(text.contains("# top comment"));
        assert!(text.contains("# inline"));
    }

    // -----------------------------------------------------------------------
    // Builder API tests
    // -----------------------------------------------------------------------

    #[test]
    fn mode_json_produces_braces() {
        let doc = Document::parse_str("foo: bar\nbaz: 1").unwrap();
        let events = doc.emitter().mode(EmitMode::Json).emit_events().unwrap();
        let json = events_to_string(&events);
        assert!(json.contains('{'), "expected JSON object, got: {json}");
    }

    #[test]
    fn indent_changes_nesting() {
        let doc = Document::parse_str("parent:\n  child: value").unwrap();
        let events = doc
            .emitter()
            .mode(EmitMode::Block)
            .indent(4)
            .emit_events()
            .unwrap();
        let out = events_to_string(&events);
        // With indent=4 in block mode, "child" should be indented by 4 spaces
        assert!(
            out.contains("    child"),
            "expected 4-space indent, got:\n{out}"
        );
    }

    #[test]
    fn sort_keys_reorders() {
        let doc = Document::parse_str("z: 1\na: 2\nm: 3").unwrap();
        let events = doc.emitter().sort_keys(true).emit_events().unwrap();
        let out = events_to_string(&events);
        let a_pos = out.find("a:").expect("missing 'a:'");
        let m_pos = out.find("m:").expect("missing 'm:'");
        let z_pos = out.find("z:").expect("missing 'z:'");
        assert!(a_pos < m_pos, "a should come before m");
        assert!(m_pos < z_pos, "m should come before z");
    }

    #[test]
    fn builder_chaining_compiles() {
        let doc = Document::parse_str("key: val").unwrap();
        let _events = doc
            .emitter()
            .mode(EmitMode::Block)
            .indent(2)
            .width(120)
            .sort_keys(false)
            .output_comments(true)
            .doc_start_mark(Toggle::Off)
            .doc_end_mark(Toggle::Auto)
            .emit_events()
            .unwrap();
    }

    #[test]
    fn no_ending_newline_strips_trailing() {
        let doc = Document::parse_str("foo: bar").unwrap();
        let events = doc.emitter().no_ending_newline(true).emit_events().unwrap();
        let out = events_to_string(&events);
        assert!(
            !out.ends_with('\n'),
            "expected no trailing newline, got: {out:?}"
        );
    }
}
