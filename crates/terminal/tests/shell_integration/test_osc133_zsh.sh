#!/usr/bin/env zsh
# Test harness for OSC 133 shell integration hooks (zsh).
#
# Simulates prompt cycles by calling precmd/preexec functions and captures
# the OSC 133 marks emitted. Runs each scenario against all fixture
# implementations (wezterm, ghostty, zed) and reports pass/fail.
#
# Usage:
#   ./test_osc133_zsh.sh [fixture_name]
#
# If fixture_name is given, only that fixture is tested. Otherwise all are run.
# Exit code is 0 if all tests pass, 1 otherwise.

emulate -L zsh -o no_aliases -o no_glob

SCRIPT_DIR="${0:A:h}"
FIXTURES_DIR="${SCRIPT_DIR}/fixtures"

# ── Colour output ────────────────────────────────────────────────────────────

if [[ -t 1 ]]; then
    RED=$'\e[31m' GREEN=$'\e[32m' YELLOW=$'\e[33m' CYAN=$'\e[36m' BOLD=$'\e[1m' RESET=$'\e[0m'
else
    RED="" GREEN="" YELLOW="" CYAN="" BOLD="" RESET=""
fi

# ── Globals ──────────────────────────────────────────────────────────────────

typeset -i PASS_COUNT=0
typeset -i FAIL_COUNT=0

# Accumulated marks for the current scenario. Each entry is a single letter
# (A, B, C, D) or a payload like "D;0".
typeset -a CAPTURED_MARKS

# Temp file used to capture stdout from functions that run in the current
# shell. We must NOT use $(...) subshells because precmd/preexec hooks
# modify PS1 and other variables in-place.
typeset _CAPTURE_FILE="${TMPDIR:-/tmp}/osc133_test.$$.capture"

# ── Mark capture ─────────────────────────────────────────────────────────────

# Extract OSC 133 marks from a raw byte string.
# Appends to CAPTURED_MARKS.
_extract_marks() {
    local raw="$1"
    local rest="$raw"
    while [[ "$rest" == *$'\e]133;'* ]]; do
        rest="${rest#*$'\e]133;'}"
        local payload="${rest%%$'\a'*}"
        # Primary mark letter.
        local letter="${payload%%[; ]*}"
        if [[ "$letter" == "D" && "$payload" == D\;* ]]; then
            CAPTURED_MARKS+=("$payload")
        elif [[ "$letter" == "D" ]]; then
            CAPTURED_MARKS+=("D")
        elif [[ "$letter" == "P" ]]; then
            # Wezterm uses 133;P;k=i instead of 133;A inside PS1.
            # Treat as A-family mark for counting purposes.
            CAPTURED_MARKS+=("A*")
        else
            CAPTURED_MARKS+=("$letter")
        fi
        rest="${rest#*$'\a'}"
    done
}

# Read the capture file, extract marks from it, and clear the file.
_flush_capture() {
    if [[ -s "$_CAPTURE_FILE" ]]; then
        _extract_marks "$(<"$_CAPTURE_FILE")"
    fi
    : > "$_CAPTURE_FILE"
}

# ── Prompt simulation ────────────────────────────────────────────────────────
#
# All functions redirect stdout to $_CAPTURE_FILE so that printf/print output
# from the hooks is collected WITHOUT running in a subshell. This preserves
# PS1 modifications and variable changes made by the hooks.

# Call all registered precmd functions, then expand PS1 (which may contain
# embedded escape sequences from hooks that wrap it).
_run_precmd() {
    : > "$_CAPTURE_FILE"
    local fn
    for fn in "${precmd_functions[@]}"; do
        if (( $+functions[$fn] )); then
            "$fn" >> "$_CAPTURE_FILE" 2>/dev/null
        fi
    done
    # After precmd, zsh displays the prompt. Expand PS1 and capture any
    # embedded escape sequences.
    print -nP -- "$PS1" >> "$_CAPTURE_FILE" 2>/dev/null
    _flush_capture
}

# Simulate zle-line-init (fires once after prompt is drawn, before user types).
_run_line_init() {
    : > "$_CAPTURE_FILE"
    # Zed's hook.
    if (( $+functions[__zed_semantic_line_init] )); then
        __zed_semantic_line_init >> "$_CAPTURE_FILE" 2>/dev/null
    fi
    # Ghostty installs a zle-line-init wrapper; in our test fixtures it may
    # define _ghostty_zle_line_init instead.
    if (( $+functions[_ghostty_zle_line_init] )); then
        _ghostty_zle_line_init >> "$_CAPTURE_FILE" 2>/dev/null
    fi
    _flush_capture
}

# Call all registered preexec functions.
_run_preexec() {
    local cmd="${1:-ls}"
    : > "$_CAPTURE_FILE"
    local fn
    for fn in "${preexec_functions[@]}"; do
        if (( $+functions[$fn] )); then
            "$fn" "$cmd" >> "$_CAPTURE_FILE" 2>/dev/null
        fi
    done
    _flush_capture
}

# ── Reset state ──────────────────────────────────────────────────────────────

_reset_state() {
    precmd_functions=()
    preexec_functions=()

    local fn
    for fn in \
        __wezterm_semantic_precmd __wezterm_semantic_preexec \
        _ghostty_precmd _ghostty_preexec _ghostty_deferred_init \
        _ghostty_zle_line_init \
        __zed_semantic_precmd __zed_semantic_preexec __zed_semantic_line_init \
        __test_plugin_precmd \
        zle-line-init; do
        unfunction "$fn" 2>/dev/null
    done

    unset __wezterm_semantic_precmd_executing __wezterm_save_ps1 __wezterm_save_ps2 __wezterm_check_ps1
    unset _ghostty_state _ghostty_fd
    unset __ZED_OSC133_INSTALLED __zed_semantic_executing

    CAPTURED_MARKS=()
    PS1='test-prompt %# '
    PS2='%_> '
    : > "$_CAPTURE_FILE"
}

# ── Helpers ──────────────────────────────────────────────────────────────────

# Count how many times a given mark letter appears in CAPTURED_MARKS.
_count_mark() {
    local target="$1"
    local -i count=0
    local m
    for m in "${CAPTURED_MARKS[@]}"; do
        case "$target" in
            "A") [[ "$m" == "A" || "$m" == "A*" ]] && (( count++ )) ;;
            *)   [[ "$m" == "$target"* ]] && (( count++ )) ;;
        esac
    done
    print -n $count
}

# ── Assertions ───────────────────────────────────────────────────────────────

# Pass/fail with message.
_pass() { print "  ${GREEN}✓${RESET} $1"; (( PASS_COUNT++ )); }
_fail() { print "  ${RED}✗${RESET} $1"; (( FAIL_COUNT++ )); }

# Assert that the number of B marks per cycle is stable (no accumulation)
# and that there is at least 1 B per cycle.
_assert_b_stable() {
    local test_name="$1"
    shift
    local -a counts
    counts=("$@")

    local -i ok=1
    local -i expected_b="${counts[1]}"

    # All cycles should have the same number of B marks.
    local -i i
    for (( i = 1; i <= ${#counts}; i++ )); do
        if (( counts[$i] != expected_b )); then
            ok=0
            break
        fi
    done

    # And there should be at least 1 B per cycle.
    if (( expected_b < 1 )); then
        ok=0
    fi

    if (( ok )); then
        _pass "${test_name} (${expected_b} B per cycle)"
    else
        _fail "${test_name}"
        print "    B counts per cycle: ${counts[*]}  (should all be equal and >= 1)"
    fi
}

# ── Scenarios ────────────────────────────────────────────────────────────────
#
# Tests focus on PROPERTIES that a correct implementation must have, rather
# than exact mark sequences (which legitimately differ across implementations).

# Scenario: sanity — fixture produces at least A, B, and C in one cycle.
scenario_produces_marks() {
    local fixture="$1"
    CAPTURED_MARKS=()

    _run_precmd
    _run_line_init
    _run_preexec "echo hello"

    local -i has_a=$(_count_mark A)
    local -i has_b=$(_count_mark B)
    local -i has_c=$(_count_mark C)

    if (( has_a >= 1 && has_b >= 1 && has_c >= 1 )); then
        _pass "produces A, B, C marks"
    else
        _fail "missing marks: A=$has_a B=$has_b C=$has_c"
        print "    marks: ${CAPTURED_MARKS[*]}"
    fi
}

# Scenario: ordering — within each cycle A comes before B comes before C.
scenario_ordering() {
    local fixture="$1"
    CAPTURED_MARKS=()

    _run_precmd
    _run_line_init
    _run_preexec "ls"
    _run_precmd
    _run_line_init

    # Find positions of first A, first B, first C.
    local -i pos_a=0 pos_b=0 pos_c=0
    local -i idx=0
    local m
    for m in "${CAPTURED_MARKS[@]}"; do
        (( idx++ ))
        if (( pos_a == 0 )) && [[ "$m" == "A" || "$m" == "A*" ]]; then pos_a=$idx; fi
        if (( pos_b == 0 )) && [[ "$m" == "B" ]]; then pos_b=$idx; fi
        if (( pos_c == 0 )) && [[ "$m" == "C" ]]; then pos_c=$idx; fi
    done

    if (( pos_a > 0 && pos_b > 0 && pos_c > 0 && pos_a < pos_b && pos_b < pos_c )); then
        _pass "ordering: A before B before C"
    else
        _fail "ordering: A@$pos_a B@$pos_b C@$pos_c"
        print "    marks: ${CAPTURED_MARKS[*]}"
    fi
}

# Scenario: D mark only after a command has run.
scenario_d_after_command() {
    local fixture="$1"
    CAPTURED_MARKS=()

    # First prompt — no command has run yet, so no D.
    _run_precmd
    _run_line_init

    local -i d_before=$(_count_mark D)

    # Run a command.
    _run_preexec "ls"

    # Second prompt — should emit D.
    _run_precmd
    _run_line_init

    local -i d_after=$(_count_mark D)

    if (( d_before == 0 && d_after >= 1 )); then
        _pass "D mark: none before first command, present after"
    else
        _fail "D mark: before=$d_before (want 0), after=$d_after (want ≥1)"
        print "    marks: ${CAPTURED_MARKS[*]}"
    fi
}

# Scenario: B count stays at exactly 1 per prompt over 5 normal cycles.
scenario_b_stable_basic() {
    local fixture="$1"
    local -a b_counts

    local -i i
    for (( i = 1; i <= 5; i++ )); do
        CAPTURED_MARKS=()
        _run_precmd
        _run_line_init

        b_counts+=($(_count_mark B))

        CAPTURED_MARKS=()
        _run_preexec "cmd_$i"
    done

    _assert_b_stable "B stable (basic)" "${b_counts[@]}"
}

# Scenario: plugin REPLACES PS1 entirely between precmd and preexec.
# This simulates starship, oh-my-posh, or a theme that rebuilds PS1.
scenario_ps1_replaced_by_plugin() {
    local fixture="$1"
    local -a b_counts

    local -i i
    for (( i = 1; i <= 5; i++ )); do
        CAPTURED_MARKS=()
        _run_precmd
        _run_line_init
        b_counts+=($(_count_mark B))

        # Plugin REPLACES PS1 (destroying any wrapping).
        PS1="plugin-replaced-v${i} %# "

        CAPTURED_MARKS=()
        _run_preexec "ls"
    done

    _assert_b_stable "B stable (PS1 replaced by plugin)" "${b_counts[@]}"
}

# Scenario: plugin APPENDS to PS1 after precmd. This preserves our wrapping
# but changes the value, breaking the save/restore equality check. This is
# the trigger for the accumulation bug in wezterm's approach.
scenario_ps1_appended_by_plugin() {
    local fixture="$1"
    local -a b_counts

    # Install a fake plugin that appends a git-status segment to PS1 after
    # the real hooks have run. Since precmd_functions are called in order,
    # we append our plugin AFTER sourcing the fixture so it runs last.
    __test_plugin_precmd() {
        PS1="$PS1 [git:main]"
    }
    precmd_functions+=(__test_plugin_precmd)

    local -i i
    for (( i = 1; i <= 8; i++ )); do
        CAPTURED_MARKS=()
        _run_precmd
        _run_line_init
        b_counts+=($(_count_mark B))

        CAPTURED_MARKS=()
        _run_preexec "ls"
    done

    _assert_b_stable "B stable (PS1 appended by plugin)" "${b_counts[@]}"
}

# Scenario: Ctrl-C — precmd fires twice without an intervening preexec.
scenario_ctrl_c() {
    local fixture="$1"
    CAPTURED_MARKS=()

    # First normal prompt.
    _run_precmd
    _run_line_init

    # User presses Ctrl-C — precmd fires again without preexec.
    _run_precmd
    _run_line_init

    # User types a command this time.
    _run_preexec "ls"

    # Command finishes, next prompt.
    _run_precmd
    _run_line_init

    local -i b_count=$(_count_mark B)

    # 3 prompts displayed → 3 B marks.
    if (( b_count == 3 )); then
        _pass "ctrl-c recovery (3 B across 3 prompts)"
    else
        _fail "ctrl-c: B count=$b_count (expected 3)"
        print "    marks: ${CAPTURED_MARKS[*]}"
    fi
}

# Scenario: double-sourcing the hook script should not double-register.
scenario_double_source() {
    local fixture="$1"
    local fixture_file="${FIXTURES_DIR}/${fixture}_zsh.sh"

    # Source the fixture a second time.
    source "$fixture_file" 2>/dev/null

    # Count how many times our precmd is registered.
    local -i precmd_count=0
    local fn
    for fn in "${precmd_functions[@]}"; do
        case "$fixture" in
            wezterm) [[ "$fn" == __wezterm_semantic_precmd ]] && (( precmd_count++ )) ;;
            ghostty) [[ "$fn" == _ghostty_precmd ]] && (( precmd_count++ )) ;;
            zed)     [[ "$fn" == __zed_semantic_precmd ]] && (( precmd_count++ )) ;;
        esac
    done

    if (( precmd_count <= 1 )); then
        _pass "double-source does not duplicate hooks"
    else
        _fail "double-source: precmd registered $precmd_count times"
    fi
}

# Scenario: long session — 20 cycles, PS1 interference every other cycle.
scenario_long_session() {
    local fixture="$1"
    local -a b_counts
    local -i n=20

    local -i i
    for (( i = 1; i <= n; i++ )); do
        CAPTURED_MARKS=()
        _run_precmd
        _run_line_init
        b_counts+=($(_count_mark B))

        # Every other cycle, a plugin replaces PS1.
        if (( i % 2 == 0 )); then
            PS1="dynamic-prompt-v${i} %# "
        fi

        CAPTURED_MARKS=()
        _run_preexec "cmd_$i"
    done

    _assert_b_stable "long session (${n} cycles, PS1 interference)" "${b_counts[@]}"
}

# Scenario: prompt redraw (SIGWINCH / window resize). Zsh re-evaluates PS1
# without running precmd or zle-line-init. Marks embedded in PS1 are
# re-emitted; marks from printf/zle-line-init are NOT. This is the key
# advantage of ghostty's PS1-embedding approach.
scenario_prompt_redraw() {
    local fixture="$1"
    CAPTURED_MARKS=()

    # Normal prompt cycle — establishes marks.
    _run_precmd
    _run_line_init

    # Now simulate a window resize: zsh redraws the prompt by re-expanding
    # PS1, but does NOT call precmd or zle-line-init again.
    CAPTURED_MARKS=()
    print -nP -- "$PS1" >> "$_CAPTURE_FILE" 2>/dev/null
    _flush_capture

    local -i has_a=$(_count_mark A)
    local -i has_b=$(_count_mark B)

    if (( has_a >= 1 && has_b >= 1 )); then
        _pass "prompt redraw: marks survive in PS1 (A=$has_a B=$has_b)"
    else
        _fail "prompt redraw: marks lost (A=$has_a B=$has_b)"
        print "    marks after redraw: ${CAPTURED_MARKS[*]}"
        print "    (PS1-embedded marks survive resize; printf/zle marks do not)"
    fi
}

# Scenario: async plugin calls `zle reset-prompt` to update the prompt after
# background work completes (e.g. powerlevel10k, pure). This re-evaluates
# PS1 without precmd. Same as resize — only PS1-embedded marks survive.
scenario_reset_prompt() {
    local fixture="$1"
    CAPTURED_MARKS=()

    _run_precmd
    _run_line_init

    # Simulate an async plugin updating PS1 and then triggering reset-prompt.
    # The plugin prepends a git status segment to PS1.
    PS1="[async-git:main] $PS1"

    # reset-prompt re-expands PS1 — no precmd, no zle-line-init.
    CAPTURED_MARKS=()
    print -nP -- "$PS1" >> "$_CAPTURE_FILE" 2>/dev/null
    _flush_capture

    local -i has_a=$(_count_mark A)
    local -i has_b=$(_count_mark B)

    if (( has_a >= 1 && has_b >= 1 )); then
        _pass "reset-prompt: marks survive async PS1 update (A=$has_a B=$has_b)"
    else
        _fail "reset-prompt: marks lost after async PS1 update (A=$has_a B=$has_b)"
        print "    marks after reset-prompt: ${CAPTURED_MARKS[*]}"
    fi
}

# Scenario: multiple rapid redraws (e.g. resize during typing, SIGCHLD with
# notify). Marks embedded in PS1 should not accumulate across redraws.
scenario_redraw_no_accumulation() {
    local fixture="$1"
    CAPTURED_MARKS=()

    _run_precmd
    _run_line_init

    # Simulate 5 consecutive redraws without precmd.
    local -a b_counts
    local -i i
    for (( i = 1; i <= 5; i++ )); do
        CAPTURED_MARKS=()
        print -nP -- "$PS1" >> "$_CAPTURE_FILE" 2>/dev/null
        _flush_capture
        b_counts+=($(_count_mark B))
    done

    # If B counts grow, marks are accumulating in PS1 across redraws.
    local -i first_b="${b_counts[1]}"
    local -i ok=1
    for (( i = 1; i <= ${#b_counts}; i++ )); do
        if (( b_counts[$i] != first_b )); then
            ok=0
            break
        fi
    done

    if (( ok )); then
        _pass "redraw: B count stable across 5 redraws ($first_b per redraw)"
    else
        _fail "redraw: B accumulates across redraws"
        print "    B counts per redraw: ${b_counts[*]}"
    fi
}

# Scenario: long session with PS1 APPEND interference.
scenario_long_session_append() {
    local fixture="$1"
    local -a b_counts
    local -i n=20

    __test_plugin_precmd() {
        PS1="$PS1 [git:main]"
    }
    precmd_functions+=(__test_plugin_precmd)

    local -i i
    for (( i = 1; i <= n; i++ )); do
        CAPTURED_MARKS=()
        _run_precmd
        _run_line_init
        b_counts+=($(_count_mark B))

        CAPTURED_MARKS=()
        _run_preexec "cmd_$i"
    done

    _assert_b_stable "long session (${n} cycles, PS1 appended)" "${b_counts[@]}"
}

# ── Runner ───────────────────────────────────────────────────────────────────

run_fixture() {
    local fixture="$1"
    local fixture_file="${FIXTURES_DIR}/${fixture}_zsh.sh"

    if [[ ! -f "$fixture_file" ]]; then
        print "${RED}Fixture not found:${RESET} $fixture_file"
        return 1
    fi

    print "${BOLD}${CYAN}═══ ${fixture} ═══${RESET}"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_produces_marks "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_ordering "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_d_after_command "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_b_stable_basic "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_ps1_replaced_by_plugin "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_ps1_appended_by_plugin "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_ctrl_c "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_double_source "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_long_session "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_long_session_append "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_prompt_redraw "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_reset_prompt "$fixture"

    _reset_state
    source "$fixture_file" 2>/dev/null
    scenario_redraw_no_accumulation "$fixture"

    print ""
}

# ── Cleanup ──────────────────────────────────────────────────────────────────

_cleanup() {
    rm -f "$_CAPTURE_FILE"
}
trap _cleanup EXIT INT TERM

# ── Main ─────────────────────────────────────────────────────────────────────

main() {
    local -a fixtures
    if [[ -n "$1" ]]; then
        fixtures=("$1")
    else
        fixtures=(wezterm ghostty zed)
    fi

    print "${BOLD}OSC 133 Shell Integration Tests (zsh)${RESET}"
    print "Fixtures: ${fixtures[*]}"
    print ""

    local fixture
    for fixture in "${fixtures[@]}"; do
        run_fixture "$fixture"
    done

    print "────────────────────────────────"
    print "${GREEN}Passed: ${PASS_COUNT}${RESET}  ${RED}Failed: ${FAIL_COUNT}${RESET}"

    if (( FAIL_COUNT > 0 )); then
        return 1
    fi
    return 0
}

main "$@"