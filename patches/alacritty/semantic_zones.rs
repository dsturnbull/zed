// =============================================================================
// OSC 133 Semantic Zones — additions for alacritty_terminal/src/term/mod.rs
//
// This file contains:
//   1. Type definitions to add near the top of term/mod.rs
//   2. New field for `struct Term<T>`
//   3. Initialization in `Term::new()`
//   4. impl<T> Term<T> methods for handling OSC 133 marks and querying zones
//   5. Call sites in existing methods (scroll, resize, reset)
//   6. Unit tests
//
// See README.md for integration instructions.
// =============================================================================

// -----------------------------------------------------------------------------
// 1. Type definitions — add after the existing `ClipboardType` enum
// -----------------------------------------------------------------------------

/// The type of a semantic zone derived from shell integration marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticZoneType {
    /// The shell prompt (between OSC 133;A and OSC 133;B).
    Prompt,
    /// User input on the command line (between OSC 133;B and OSC 133;C).
    Input,
    /// Command output (between OSC 133;C and OSC 133;D or the next OSC 133;A).
    Output,
}

/// A semantic zone in the terminal grid, tracking shell integration boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticZone {
    /// The type of this zone.
    pub zone_type: SemanticZoneType,
    /// Start point in the grid (inclusive).
    pub start: Point,
    /// End point in the grid (inclusive).
    pub end: Point,
}

/// The type of a shell integration mark (one of the four OSC 133 subcommands).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticMarkType {
    /// OSC 133 ; A — prompt start.
    PromptStart,
    /// OSC 133 ; B — command start (user pressed enter).
    CommandStart,
    /// OSC 133 ; C — command output start.
    OutputStart,
    /// OSC 133 ; D ; <exit_code> — command finished.
    CommandFinished(Option<i32>),
}

/// A single shell integration mark, recording the cursor position at the time
/// the escape sequence was received and the type of transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticMark {
    pub point: Point,
    pub mark_type: SemanticMarkType,
}

// -----------------------------------------------------------------------------
// 2. New field for `struct Term<T>` — add after the `config` field
// -----------------------------------------------------------------------------
//
//     /// Marks placed by shell integration (OSC 133).
//     /// Each mark records the grid point and the type of transition.
//     semantic_marks: Vec<SemanticMark>,

// -----------------------------------------------------------------------------
// 3. Initialization — add in `Term::new()` struct literal
// -----------------------------------------------------------------------------
//
//     semantic_marks: Vec::new(),

// -----------------------------------------------------------------------------
// 4. impl<T> Term<T> methods
// -----------------------------------------------------------------------------

// Add this impl block (or merge into the existing `impl<T> Term<T>`).

/// Methods for OSC 133 shell integration support.
impl<T> Term<T> {
    /// Handle an OSC 133 subcommand received from the PTY.
    ///
    /// `payload` is the portion after `133;`, e.g. `"A"`, `"B"`, `"C"`,
    /// `"D;0"`, `"D"`.
    pub fn handle_osc_133(&mut self, payload: &str) {
        // Ignore shell integration marks on the alternate screen — full-screen
        // applications (vim, less, etc.) don't use shell integration.
        if self.mode.contains(TermMode::ALT_SCREEN) {
            return;
        }

        let point = self.grid.cursor.point;
        let mark_type = match payload {
            "A" => SemanticMarkType::PromptStart,
            "B" => SemanticMarkType::CommandStart,
            "C" => SemanticMarkType::OutputStart,
            _ if payload.starts_with('D') => {
                let exit_code = payload
                    .strip_prefix("D;")
                    .and_then(|s| s.trim().parse::<i32>().ok());
                SemanticMarkType::CommandFinished(exit_code)
            },
            _ => return,
        };

        self.semantic_marks.push(SemanticMark { point, mark_type });
    }

    /// Returns semantic zones derived from the stored shell integration marks.
    ///
    /// Walks the mark list and constructs contiguous zones from adjacent pairs:
    ///   - `PromptStart` → `CommandStart` = `Prompt` zone
    ///   - `CommandStart` → `OutputStart` = `Input` zone
    ///   - `OutputStart` → next `PromptStart` or `CommandFinished` = `Output` zone
    pub fn semantic_zones(&self) -> Vec<SemanticZone> {
        let mut zones = Vec::new();
        let marks = &self.semantic_marks;

        if marks.is_empty() {
            return zones;
        }

        for i in 0..marks.len() {
            let mark = &marks[i];
            // Find the end point: the start of the next mark, or the current
            // cursor position if this is the last mark.
            let end_point = if i + 1 < marks.len() {
                // End just before the next mark. Use the next mark's point
                // as the exclusive boundary; we subtract one column if
                // possible so the zone end is inclusive.
                let next_point = marks[i + 1].point;
                if next_point.column > Column(0) {
                    Point::new(next_point.line, next_point.column - 1)
                } else if next_point.line > self.topmost_line() {
                    // Wrap to the end of the previous line.
                    Point::new(next_point.line - 1i32, self.last_column())
                } else {
                    next_point
                }
            } else {
                self.grid.cursor.point
            };

            // Only create a zone if start <= end.
            if mark.point > end_point {
                continue;
            }

            let zone_type = match mark.mark_type {
                SemanticMarkType::PromptStart => SemanticZoneType::Prompt,
                SemanticMarkType::CommandStart => SemanticZoneType::Input,
                SemanticMarkType::OutputStart => SemanticZoneType::Output,
                // CommandFinished doesn't start a zone; it only terminates
                // the preceding output zone (handled by the end-point logic
                // above). Skip it.
                SemanticMarkType::CommandFinished(_) => continue,
            };

            zones.push(SemanticZone {
                zone_type,
                start: mark.point,
                end: end_point,
            });
        }

        zones
    }

    /// Returns the most recent command output zone, if any.
    pub fn last_command_output(&self) -> Option<SemanticZone> {
        self.semantic_zones()
            .into_iter()
            .rev()
            .find(|zone| zone.zone_type == SemanticZoneType::Output)
    }

    /// Extract the command text from the `Input` zone that immediately
    /// precedes the given output zone.
    ///
    /// Returns `None` if no matching input zone is found or if the zone
    /// contents are empty.
    pub fn command_for_output(&self, output_zone: &SemanticZone) -> Option<String> {
        let zones = self.semantic_zones();
        let output_index = zones.iter().position(|z| z == output_zone)?;
        if output_index == 0 {
            return None;
        }

        let preceding = &zones[output_index - 1];
        if preceding.zone_type != SemanticZoneType::Input {
            return None;
        }

        let text = self.bounds_to_string(preceding.start, preceding.end);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }

    /// Read-only access to the raw semantic marks (for serialization, debug, etc.).
    pub fn semantic_marks(&self) -> &[SemanticMark] {
        &self.semantic_marks
    }

    /// Returns the exit code from the most recent `CommandFinished` mark, if any.
    pub fn last_exit_code(&self) -> Option<Option<i32>> {
        self.semantic_marks
            .iter()
            .rev()
            .find_map(|mark| match mark.mark_type {
                SemanticMarkType::CommandFinished(code) => Some(code),
                _ => None,
            })
    }

    /// Adjust semantic mark positions after a scroll-up operation.
    ///
    /// When the grid scrolls up, lines in the affected region move upward by
    /// `delta` lines. Marks whose lines fall below the history limit are
    /// discarded.
    ///
    /// Call this from `scroll_up_relative()` after the selection adjustment:
    /// ```ignore
    /// self.adjust_semantic_marks_for_scroll(&region, lines as i32);
    /// ```
    pub fn adjust_semantic_marks_for_scroll(&mut self, region: &Range<Line>, delta: i32) {
        let topmost = self.topmost_line();

        self.semantic_marks.retain_mut(|mark| {
            if mark.point.line >= region.start && mark.point.line < region.end {
                mark.point.line -= delta;
            }
            // Discard marks that have scrolled past the history limit.
            mark.point.line >= topmost
        });
    }
}

// -----------------------------------------------------------------------------
// 5. Call sites in existing `Term<T>` methods
// -----------------------------------------------------------------------------
//
// (a) In `scroll_up_relative()`, after:
//     ```
//     self.selection = self.selection.take().and_then(|s| s.rotate(self, &region, lines as i32));
//     ```
//     Add:
//     ```
//     self.adjust_semantic_marks_for_scroll(&region, lines as i32);
//     ```
//
// (b) In `resize()`, after the selection invalidation block
//     (`if old_cols != num_cols { self.selection = None; ... }`),
//     add:
//     ```
//     self.semantic_marks.clear();
//     ```
//
// (c) In `reset_state()`, add:
//     ```
//     self.semantic_marks.clear();
//     ```
//
// (d) In `swap_alt()`, the marks live on the primary grid only. No action
//     needed since we skip marks when ALT_SCREEN is active in handle_osc_133.

// -----------------------------------------------------------------------------
// 6. Unit tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod semantic_zone_tests {
    use super::*;

    // These tests assume the test helpers from term/mod.rs are in scope.
    // Adjust imports as needed when integrating.
    use crate::event::VoidListener;
    use crate::index::{Column, Line, Point};
    use crate::term::test::TermSize;
    use crate::term::Config;
    use crate::vte::ansi::Handler;

    fn make_term(cols: usize, lines: usize) -> Term<VoidListener> {
        let size = TermSize::new(cols, lines);
        Term::new(Config::default(), &size, VoidListener)
    }

    #[test]
    fn osc133_basic_zone_construction() {
        let mut term = make_term(80, 24);

        // Simulate: prompt, then command, then output, then finished.
        // Prompt at (0, 0).
        term.handle_osc_133("A");

        // Type some prompt text.
        for c in "$ ".chars() {
            term.input(c);
        }

        // Command start at current cursor.
        term.handle_osc_133("B");

        // Type command text.
        for c in "ls -la".chars() {
            term.input(c);
        }

        // Output start.
        term.handle_osc_133("C");

        // Some output.
        term.linefeed();
        term.carriage_return();
        for c in "file1.txt".chars() {
            term.input(c);
        }

        // Command finished with exit code 0.
        term.handle_osc_133("D;0");

        let zones = term.semantic_zones();
        assert_eq!(zones.len(), 3, "Expected 3 zones, got: {:?}", zones);

        assert_eq!(zones[0].zone_type, SemanticZoneType::Prompt);
        assert_eq!(zones[0].start, Point::new(Line(0), Column(0)));

        assert_eq!(zones[1].zone_type, SemanticZoneType::Input);

        assert_eq!(zones[2].zone_type, SemanticZoneType::Output);
    }

    #[test]
    fn osc133_last_command_output() {
        let mut term = make_term(80, 24);

        // First command cycle.
        term.handle_osc_133("A");
        term.handle_osc_133("B");
        term.handle_osc_133("C");
        for c in "output1".chars() {
            term.input(c);
        }
        term.handle_osc_133("D;0");

        // Second command cycle.
        term.handle_osc_133("A");
        term.handle_osc_133("B");
        term.handle_osc_133("C");
        for c in "output2".chars() {
            term.input(c);
        }
        term.handle_osc_133("D;1");

        let last_output = term.last_command_output();
        assert!(last_output.is_some());
        let zone = last_output.expect("should have last output zone");
        assert_eq!(zone.zone_type, SemanticZoneType::Output);

        // The last exit code should be 1.
        assert_eq!(term.last_exit_code(), Some(Some(1)));
    }

    #[test]
    fn osc133_command_finished_without_exit_code() {
        let mut term = make_term(80, 24);

        term.handle_osc_133("A");
        term.handle_osc_133("B");
        term.handle_osc_133("C");
        term.handle_osc_133("D");

        assert_eq!(term.last_exit_code(), Some(None));
    }

    #[test]
    fn osc133_unknown_subcommand_ignored() {
        let mut term = make_term(80, 24);
        let before = term.semantic_marks().len();
        term.handle_osc_133("Z");
        term.handle_osc_133("");
        term.handle_osc_133("X;foo");
        assert_eq!(term.semantic_marks().len(), before);
    }

    #[test]
    fn osc133_alt_screen_ignored() {
        let mut term = make_term(80, 24);

        // Switch to alt screen.
        term.swap_alt();
        assert!(term.mode().contains(TermMode::ALT_SCREEN));

        term.handle_osc_133("A");
        assert!(term.semantic_marks().is_empty());

        // Switch back.
        term.swap_alt();
        assert!(!term.mode().contains(TermMode::ALT_SCREEN));

        term.handle_osc_133("A");
        assert_eq!(term.semantic_marks().len(), 1);
    }

    #[test]
    fn osc133_reset_clears_marks() {
        let mut term = make_term(80, 24);
        term.handle_osc_133("A");
        term.handle_osc_133("B");
        assert_eq!(term.semantic_marks().len(), 2);

        term.reset_state();
        assert!(term.semantic_marks().is_empty());
    }

    #[test]
    fn osc133_command_for_output() {
        let mut term = make_term(80, 24);

        // Prompt.
        term.handle_osc_133("A");
        for c in "$ ".chars() {
            term.input(c);
        }

        // Command: "echo hello".
        term.handle_osc_133("B");
        for c in "echo hello".chars() {
            term.input(c);
        }

        // Output.
        term.handle_osc_133("C");
        term.linefeed();
        term.carriage_return();
        for c in "hello".chars() {
            term.input(c);
        }

        term.handle_osc_133("D;0");

        let output_zone = term.last_command_output().expect("should have output zone");
        let command = term.command_for_output(&output_zone);
        assert_eq!(command.as_deref(), Some("echo hello"));
    }

    #[test]
    fn osc133_empty_zones() {
        let mut term = make_term(80, 24);
        assert!(term.semantic_zones().is_empty());
        assert!(term.last_command_output().is_none());
        assert!(term.last_exit_code().is_none());
    }

    #[test]
    fn osc133_multiple_prompt_starts() {
        // Some shells re-emit PromptStart when redrawing the prompt.
        let mut term = make_term(80, 24);

        term.handle_osc_133("A");
        for c in "$ ".chars() {
            term.input(c);
        }
        // Shell redraws prompt.
        term.handle_osc_133("A");
        for c in "$ ".chars() {
            term.input(c);
        }
        term.handle_osc_133("B");
        for c in "pwd".chars() {
            term.input(c);
        }
        term.handle_osc_133("C");

        let zones = term.semantic_zones();
        // Should have: Prompt, Prompt, Input, Output.
        assert_eq!(zones.len(), 4);
        assert_eq!(zones[0].zone_type, SemanticZoneType::Prompt);
        assert_eq!(zones[1].zone_type, SemanticZoneType::Prompt);
        assert_eq!(zones[2].zone_type, SemanticZoneType::Input);
        assert_eq!(zones[3].zone_type, SemanticZoneType::Output);
    }

    #[test]
    fn osc133_scroll_adjusts_marks() {
        let mut term = make_term(80, 10);

        term.handle_osc_133("A");
        let initial_line = term.semantic_marks()[0].point.line;

        // Generate enough newlines to cause scrolling.
        for _ in 0..15 {
            term.linefeed();
        }

        // The mark should have been adjusted to a negative line (scrollback).
        assert!(
            term.semantic_marks()[0].point.line < initial_line,
            "Mark should have scrolled into history, was {:?}",
            term.semantic_marks()[0].point.line,
        );
    }

    #[test]
    fn osc133_exit_code_parsing() {
        let mut term = make_term(80, 24);

        // Positive exit code.
        term.handle_osc_133("D;42");
        assert_eq!(term.last_exit_code(), Some(Some(42)));

        // Negative exit code (signal).
        term.handle_osc_133("D;-1");
        assert_eq!(term.last_exit_code(), Some(Some(-1)));

        // Exit code with whitespace.
        term.handle_osc_133("D; 7 ");
        assert_eq!(term.last_exit_code(), Some(Some(7)));

        // No exit code.
        term.handle_osc_133("D");
        assert_eq!(term.last_exit_code(), Some(None));

        // Invalid exit code (not a number) — treated as no code.
        term.handle_osc_133("D;abc");
        assert_eq!(term.last_exit_code(), Some(None));
    }
}