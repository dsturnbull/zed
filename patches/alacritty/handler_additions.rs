// =============================================================================
// OSC 133 Byte Scanner — additions for alacritty_terminal
//
// This file contains:
//   1. `Osc133Scanner` — a state machine that detects OSC 133 sequences in the
//      raw PTY byte stream and dispatches them to `Term::handle_osc_133()`.
//   2. Integration instructions for `event_loop.rs` and `State`.
//
// The scanner runs alongside the existing `vte::ansi::Processor`. Since the
// vte crate (0.15.0, from crates.io) has no Handler method for OSC 133, the
// Processor silently drops those sequences. This scanner catches them
// independently — there is no conflict because both see the same bytes but
// act on disjoint OSC codes.
//
// See README.md for the full integration guide.
// =============================================================================

// -----------------------------------------------------------------------------
// Option A: Place this in a new file `alacritty_terminal/src/osc133.rs`
//           and add `mod osc133;` + `pub use osc133::Osc133Scanner;` to lib.rs.
//
// Option B: Inline the scanner directly in `event_loop.rs`.
// -----------------------------------------------------------------------------

use crate::event::EventListener;
use crate::term::Term;

/// Maximum payload length we'll buffer before giving up on a malformed sequence.
const MAX_PAYLOAD_LEN: usize = 64;

/// State machine for detecting OSC 133 sequences in a raw byte stream.
///
/// Recognizes both BEL-terminated (`ESC ] 133 ; <payload> BEL`) and
/// ST-terminated (`ESC ] 133 ; <payload> ESC \`) forms.
///
/// # Usage
///
/// ```ignore
/// let mut scanner = Osc133Scanner::new();
/// // In the PTY read loop, after the normal VTE parser processes bytes:
/// scanner.advance(&mut terminal, &bytes[..n]);
/// ```
pub struct Osc133Scanner {
    state: ScanState,
    payload: Vec<u8>,
}

/// Internal states of the OSC 133 recognizer.
///
/// The state names describe what has been consumed so far:
///
/// ```text
///   Ground
///     │ ESC (0x1b)
///     ▼
///   Esc
///     │ ] (0x5d)
///     ▼
///   OscOpen
///     │ '1'
///     ▼
///   Osc1
///     │ '3'
///     ▼
///   Osc13
///     │ '3'
///     ▼
///   Osc133
///     │ ';'
///     ▼
///   Payload ──(BEL)──► dispatch ──► Ground
///     │
///     │ ESC
///     ▼
///   PayloadEsc ──('\\')──► dispatch ──► Ground
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Ground,
    Esc,
    OscOpen,
    Osc1,
    Osc13,
    Osc133,
    Payload,
    PayloadEsc,
}

impl Osc133Scanner {
    pub fn new() -> Self {
        Self {
            state: ScanState::Ground,
            payload: Vec::with_capacity(32),
        }
    }

    /// Feed raw PTY bytes through the scanner.
    ///
    /// When a complete `ESC ] 133 ; <payload> ST` sequence is found, calls
    /// `terminal.handle_osc_133(payload)` with the payload string (the part
    /// after `133;`, e.g. `"A"`, `"B"`, `"C"`, `"D;0"`).
    pub fn advance<T: EventListener>(&mut self, terminal: &mut Term<T>, bytes: &[u8]) {
        for &byte in bytes {
            self.state = match self.state {
                ScanState::Ground => {
                    if byte == 0x1b {
                        ScanState::Esc
                    } else {
                        ScanState::Ground
                    }
                },

                ScanState::Esc => {
                    if byte == b']' {
                        ScanState::OscOpen
                    } else {
                        // Not an OSC — but this ESC might start a different
                        // sequence that happens to contain `]` later. Reset.
                        ScanState::Ground
                    }
                },

                ScanState::OscOpen => {
                    if byte == b'1' {
                        ScanState::Osc1
                    } else {
                        // Some other OSC (title, color, clipboard, …). The
                        // normal VTE parser handles those. Skip to ground but
                        // watch for a new ESC that might begin an OSC 133.
                        self.reset_or_esc(byte)
                    }
                },

                ScanState::Osc1 => {
                    if byte == b'3' {
                        ScanState::Osc13
                    } else {
                        self.reset_or_esc(byte)
                    }
                },

                ScanState::Osc13 => {
                    if byte == b'3' {
                        ScanState::Osc133
                    } else {
                        self.reset_or_esc(byte)
                    }
                },

                ScanState::Osc133 => {
                    if byte == b';' {
                        self.payload.clear();
                        ScanState::Payload
                    } else {
                        // `ESC ] 133` followed by something other than `;`.
                        // Possibly `ESC ] 1337` (iTerm2 proprietary). Ignore.
                        self.reset_or_esc(byte)
                    }
                },

                ScanState::Payload => {
                    if byte == 0x07 {
                        // BEL terminator.
                        self.dispatch(terminal);
                        ScanState::Ground
                    } else if byte == 0x1b {
                        // Possible start of ST (`ESC \`).
                        ScanState::PayloadEsc
                    } else if self.payload.len() < MAX_PAYLOAD_LEN {
                        self.payload.push(byte);
                        ScanState::Payload
                    } else {
                        // Payload too long — not a valid OSC 133. Bail out.
                        self.payload.clear();
                        ScanState::Ground
                    }
                },

                ScanState::PayloadEsc => {
                    if byte == b'\\' {
                        // ST terminator (`ESC \`).
                        self.dispatch(terminal);
                        ScanState::Ground
                    } else {
                        // The ESC was not part of an ST. The payload is broken;
                        // discard it. The current byte might be `]` starting a
                        // new OSC, so re-evaluate from Esc state.
                        self.payload.clear();
                        if byte == b']' {
                            ScanState::OscOpen
                        } else {
                            ScanState::Ground
                        }
                    }
                },
            };
        }
    }

    /// Dispatch the collected payload to the terminal.
    fn dispatch<T: EventListener>(&mut self, terminal: &mut Term<T>) {
        if let Ok(payload) = std::str::from_utf8(&self.payload) {
            terminal.handle_osc_133(payload);
        }
        self.payload.clear();
    }

    /// Helper: if the unexpected byte is ESC, transition to the Esc state so
    /// we don't miss a new `ESC ]` that immediately follows. Otherwise reset
    /// to Ground.
    fn reset_or_esc(&self, byte: u8) -> ScanState {
        if byte == 0x1b {
            ScanState::Esc
        } else {
            ScanState::Ground
        }
    }
}

impl Default for Osc133Scanner {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Integration into event_loop.rs
// =============================================================================
//
// Below are the specific diffs to apply to
// `alacritty_terminal/src/event_loop.rs`.
//
// ─────────────────────────────────────────────────────────────────────────────
// DIFF 1: Add import
// ─────────────────────────────────────────────────────────────────────────────
//
// At the top of event_loop.rs, add:
//
//     use crate::osc133::Osc133Scanner;
//
// (Or, if you inlined the scanner, just reference it directly.)
//
// ─────────────────────────────────────────────────────────────────────────────
// DIFF 2: Add `scanner` field to `State`
// ─────────────────────────────────────────────────────────────────────────────
//
// In the `State` struct:
//
//     #[derive(Default)]
//     pub struct State {
//         write_list: VecDeque<Cow<'static, [u8]>>,
//         writing: Option<Writing>,
//         parser: ansi::Processor,
// +       scanner: Osc133Scanner,
//     }
//
// ─────────────────────────────────────────────────────────────────────────────
// DIFF 3: Drive the scanner in `pty_read()`
// ─────────────────────────────────────────────────────────────────────────────
//
// In `EventLoop::pty_read()`, immediately after:
//
//     // Parse the incoming bytes.
//     state.parser.advance(&mut **terminal, &buf[..unprocessed]);
//
// Add:
//
//     // Scan for OSC 133 shell integration sequences.
//     // The VTE parser silently drops these (no Handler method), so the
//     // scanner catches them independently.
//     state.scanner.advance(&mut **terminal, &buf[..unprocessed]);
//
// ─────────────────────────────────────────────────────────────────────────────
// DIFF 4 (optional): Add `mod osc133;` to lib.rs
// ─────────────────────────────────────────────────────────────────────────────
//
// In `alacritty_terminal/src/lib.rs`:
//
// +   mod osc133;
// +   pub use osc133::Osc133Scanner;
//
// If you prefer to keep the scanner private to the crate, use `pub(crate)`.
//
// =============================================================================

// =============================================================================
// Unit tests for the scanner
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::event::VoidListener;
    use crate::index::{Column, Line, Point};
    use crate::term::test::TermSize;
    use crate::term::{Config, SemanticMarkType, Term};

    fn make_term(cols: usize, lines: usize) -> Term<VoidListener> {
        let size = TermSize::new(cols, lines);
        Term::new(Config::default(), &size, VoidListener)
    }

    #[test]
    fn scanner_bel_terminated() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // ESC ] 133 ; A BEL
        scanner.advance(&mut term, b"\x1b]133;A\x07");

        let marks = term.semantic_marks();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].mark_type, SemanticMarkType::PromptStart);
        assert_eq!(marks[0].point, Point::new(Line(0), Column(0)));
    }

    #[test]
    fn scanner_st_terminated() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // ESC ] 133 ; B ESC backslash
        scanner.advance(&mut term, b"\x1b]133;B\x1b\\");

        let marks = term.semantic_marks();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].mark_type, SemanticMarkType::CommandStart);
    }

    #[test]
    fn scanner_all_subcommands() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        scanner.advance(&mut term, b"\x1b]133;A\x07");
        scanner.advance(&mut term, b"\x1b]133;B\x07");
        scanner.advance(&mut term, b"\x1b]133;C\x07");
        scanner.advance(&mut term, b"\x1b]133;D;0\x07");

        let marks = term.semantic_marks();
        assert_eq!(marks.len(), 4);
        assert_eq!(marks[0].mark_type, SemanticMarkType::PromptStart);
        assert_eq!(marks[1].mark_type, SemanticMarkType::CommandStart);
        assert_eq!(marks[2].mark_type, SemanticMarkType::OutputStart);
        assert_eq!(marks[3].mark_type, SemanticMarkType::CommandFinished(Some(0)));
    }

    #[test]
    fn scanner_exit_code_variations() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // Exit code 127.
        scanner.advance(&mut term, b"\x1b]133;D;127\x07");
        match term.semantic_marks().last().map(|m| m.mark_type) {
            Some(SemanticMarkType::CommandFinished(Some(127))) => {},
            other => panic!("Expected CommandFinished(Some(127)), got {:?}", other),
        }

        // No exit code.
        scanner.advance(&mut term, b"\x1b]133;D\x07");
        match term.semantic_marks().last().map(|m| m.mark_type) {
            Some(SemanticMarkType::CommandFinished(None)) => {},
            other => panic!("Expected CommandFinished(None), got {:?}", other),
        }

        // Negative exit code (signal).
        scanner.advance(&mut term, b"\x1b]133;D;-9\x07");
        match term.semantic_marks().last().map(|m| m.mark_type) {
            Some(SemanticMarkType::CommandFinished(Some(-9))) => {},
            other => panic!("Expected CommandFinished(Some(-9)), got {:?}", other),
        }
    }

    #[test]
    fn scanner_mixed_with_normal_output() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // Simulate a realistic byte stream: normal text interspersed with
        // shell integration sequences.
        let stream = b"hello\x1b]133;A\x07$ \x1b]133;B\x07ls\x1b]133;C\x07file1\n\x1b]133;D;0\x07";
        scanner.advance(&mut term, stream);

        let marks = term.semantic_marks();
        assert_eq!(marks.len(), 4);
    }

    #[test]
    fn scanner_split_across_reads() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // The OSC 133 sequence is split across two read() calls.
        scanner.advance(&mut term, b"\x1b]13");
        assert!(term.semantic_marks().is_empty());

        scanner.advance(&mut term, b"3;A\x07");
        assert_eq!(term.semantic_marks().len(), 1);
        assert_eq!(
            term.semantic_marks()[0].mark_type,
            SemanticMarkType::PromptStart,
        );
    }

    #[test]
    fn scanner_split_at_every_byte() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // Feed one byte at a time.
        for &byte in b"\x1b]133;C\x07" {
            scanner.advance(&mut term, &[byte]);
        }

        assert_eq!(term.semantic_marks().len(), 1);
        assert_eq!(
            term.semantic_marks()[0].mark_type,
            SemanticMarkType::OutputStart,
        );
    }

    #[test]
    fn scanner_ignores_other_osc_codes() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // OSC 0 (set title).
        scanner.advance(&mut term, b"\x1b]0;My Title\x07");
        assert!(term.semantic_marks().is_empty());

        // OSC 52 (clipboard).
        scanner.advance(&mut term, b"\x1b]52;c;dGVzdA==\x07");
        assert!(term.semantic_marks().is_empty());

        // OSC 1337 (iTerm2 proprietary — shares the "133" prefix).
        scanner.advance(&mut term, b"\x1b]1337;SetMark\x07");
        assert!(term.semantic_marks().is_empty());
    }

    #[test]
    fn scanner_ignores_unknown_subcommands() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        scanner.advance(&mut term, b"\x1b]133;Z\x07");
        assert!(term.semantic_marks().is_empty());

        scanner.advance(&mut term, b"\x1b]133;X;foo\x07");
        assert!(term.semantic_marks().is_empty());
    }

    #[test]
    fn scanner_consecutive_sequences() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // Two sequences back-to-back with no gap.
        scanner.advance(&mut term, b"\x1b]133;A\x07\x1b]133;B\x07");
        assert_eq!(term.semantic_marks().len(), 2);
    }

    #[test]
    fn scanner_esc_in_middle_of_non_osc() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // An ESC that starts something other than OSC, followed by a real
        // OSC 133. The scanner should recover.
        scanner.advance(&mut term, b"\x1b[31m\x1b]133;A\x07");
        assert_eq!(term.semantic_marks().len(), 1);
    }

    #[test]
    fn scanner_truncates_oversized_payload() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // Build a sequence with a payload exceeding MAX_PAYLOAD_LEN.
        let mut data = Vec::new();
        data.extend_from_slice(b"\x1b]133;");
        data.extend(std::iter::repeat(b'A').take(MAX_PAYLOAD_LEN + 10));
        data.push(0x07);
        scanner.advance(&mut term, &data);

        // Should be dropped (payload too long).
        assert!(term.semantic_marks().is_empty());
    }

    #[test]
    fn scanner_st_split_across_reads() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // ESC ] 133 ; A ESC  (split)  backslash
        scanner.advance(&mut term, b"\x1b]133;A\x1b");
        assert!(term.semantic_marks().is_empty());

        scanner.advance(&mut term, b"\\");
        assert_eq!(term.semantic_marks().len(), 1);
    }

    #[test]
    fn scanner_broken_st_recovers() {
        let mut term = make_term(80, 24);
        let mut scanner = Osc133Scanner::new();

        // An OSC 133 payload where the ESC is NOT followed by backslash
        // (broken ST). Then a valid sequence follows.
        scanner.advance(&mut term, b"\x1b]133;A\x1b[m\x1b]133;B\x07");

        // The first sequence (A) should be dropped, the second (B) should succeed.
        let marks = term.semantic_marks();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].mark_type, SemanticMarkType::CommandStart);
    }
}