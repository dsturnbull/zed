# OSC 133 (FinalTerm Shell Integration) Support for Alacritty Fork

## Overview

This patch adds support for OSC 133 semantic zone markers to the
`alacritty_terminal` crate. Shells (bash, zsh, fish) emit these escape
sequences to mark boundaries between prompts, user input, and command output.

### OSC 133 Escape Sequences

```
ESC ] 133 ; A ST   → Prompt start
ESC ] 133 ; B ST   → Command start (user pressed enter)
ESC ] 133 ; C ST   → Command output start
ESC ] 133 ; D ; N ST → Command finished (N = exit code)
```

ST (String Terminator) is either `ESC \` or `BEL` (`\x07`).

## Architecture

The `vte` crate (v0.15.0, from crates.io) parses escape sequences and calls
methods on the `vte::ansi::Handler` trait. The `Handler` trait has **no method
for OSC 133** and the `vte::ansi::Processor` silently drops unrecognized OSC
codes. Since `vte` is not forked, we cannot add a Handler method.

Instead, we use a **dedicated byte-level scanner** (`Osc133Scanner`) that runs
alongside the existing VTE parser. The scanner is a simple state machine that
recognizes `ESC ] 133 ; <payload> ST` patterns in the raw byte stream and
calls `Term::handle_osc_133()` directly. The normal VTE parser sees the same
bytes but ignores the OSC 133 (no Handler callback), so there is no conflict.

```
                    raw PTY bytes
                         │
                ┌────────┴────────┐
                │                 │
                ▼                 ▼
        ansi::Processor     Osc133Scanner
                │                 │
                ▼                 ▼
        Handler methods    Term::handle_osc_133()
        on Term<T>               │
                │                 ▼
                │          semantic_marks: Vec<SemanticMark>
                │                 │
                └────────┬────────┘
                         │
                         ▼
                    Term<T> state
```

## Files

### `semantic_zones.rs`

New types and `impl<T> Term<T>` methods to add to `alacritty_terminal/src/term/mod.rs`.

Contains:
- **`SemanticZoneType`** — Public enum: `Prompt`, `Input`, `Output`.
- **`SemanticZone`** — Public struct with `zone_type`, `start`, `end` points.
- **`SemanticMarkType`** — Enum for the four OSC 133 mark types.
- **`SemanticMark`** — Struct storing a grid `Point` and `SemanticMarkType`.
- **`Term::handle_osc_133()`** — Parses the OSC 133 subcommand and pushes a mark.
- **`Term::semantic_zones()`** — Derives contiguous zones from stored marks.
- **`Term::last_command_output()`** — Returns the most recent output zone.
- **`Term::command_for_output()`** — Extracts command text for a given output zone.
- **`Term::adjust_semantic_marks_for_scroll()`** — Adjusts mark positions when the grid scrolls.

### `handler_additions.rs`

The `Osc133Scanner` state machine and the integration points in `event_loop.rs`.

Contains:
- **`Osc133Scanner`** — Byte-level state machine that detects OSC 133 sequences.
- **`Osc133Scanner::advance()`** — Feeds bytes and dispatches to `Term::handle_osc_133()`.
- Documented diffs for `event_loop.rs` showing where to instantiate and drive the scanner.

## How to Apply

### Step 1: Add types and methods to `term/mod.rs`

Insert the contents of `semantic_zones.rs` into
`alacritty_terminal/src/term/mod.rs`:

1. Add the type definitions (`SemanticZoneType`, `SemanticZone`,
   `SemanticMarkType`, `SemanticMark`) near the top of the file, after the
   existing type definitions (after `ClipboardType`).

2. Add the `semantic_marks` field to `struct Term<T>`:
   ```rust
   /// Marks placed by shell integration (OSC 133).
   semantic_marks: Vec<SemanticMark>,
   ```

3. Initialize the field in `Term::new()`:
   ```rust
   semantic_marks: Vec::new(),
   ```

4. Clear marks in `Term::reset_state()`:
   ```rust
   self.semantic_marks.clear();
   ```

5. Add the scroll adjustment call in `Term::scroll_up_relative()`, after
   the existing selection adjustment:
   ```rust
   self.adjust_semantic_marks_for_scroll(&region, lines as i32);
   ```

6. Clear marks in `Term::resize()` (after the selection invalidation):
   ```rust
   self.semantic_marks.clear();
   ```

7. Add the `impl<T> Term<T>` method block from `semantic_zones.rs`.

### Step 2: Add the OSC 133 scanner to `event_loop.rs`

1. Add the `Osc133Scanner` from `handler_additions.rs` as a new file
   `alacritty_terminal/src/osc133.rs`, or inline it in `event_loop.rs`.

2. Add a `scanner: Osc133Scanner` field to the `State` struct.

3. In `EventLoop::pty_read()`, after
   `state.parser.advance(&mut **terminal, &buf[..unprocessed]);`,
   add:
   ```rust
   state.scanner.advance(&mut **terminal, &buf[..unprocessed]);
   ```

### Step 3: Re-export public types from `lib.rs`

In `alacritty_terminal/src/lib.rs`, ensure the new public types are
accessible:
```rust
pub use crate::term::{SemanticZone, SemanticZoneType};
```

## Scroll & Resize Behavior

- **Scroll up**: When lines scroll off the top into the scrollback buffer,
  mark line numbers are decremented. Marks that scroll past the history limit
  are discarded.
- **Resize**: All marks are cleared. Grid reflow invalidates stored positions
  and shells typically re-emit prompts after a resize anyway.
- **Alt screen**: Marks are only tracked on the primary screen. Switching to
  the alt screen does not affect marks (full-screen apps like vim don't use
  shell integration).
- **Reset**: `reset_state()` clears all marks.

## Testing

After applying, verify with:

```bash
# In a terminal running zsh/bash/fish with shell integration enabled:
printf '\e]133;A\a'       # prompt start
printf 'my-prompt> '
printf '\e]133;B\a'       # command start
printf 'ls -la'
printf '\e]133;C\a'       # output start
printf 'file1\nfile2\n'
printf '\e]133;D;0\a'     # command finished, exit 0
```

The `Term::semantic_zones()` method should return zones matching these
boundaries. A unit test is included in `semantic_zones.rs`.