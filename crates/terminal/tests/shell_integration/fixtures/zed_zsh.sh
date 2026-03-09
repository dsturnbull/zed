# Zed zsh OSC 133 hooks
#
# Uses ghostty's pattern-match-and-strip strategy for PS1:
#   - precmd: emit D (if command ran), then add A and B marks to PS1 using
#     pattern matching to avoid double-insertion
#   - preexec: strip marks from PS1 via pattern substitution, emit C
#
# This gives us:
#   - No accumulation: pattern matching prevents double-wrapping
#   - Redraw survival: marks are embedded in PS1, so they're re-emitted on
#     window resize, zle reset-prompt, and SIGCHLD with notify
#   - Plugin-safe: stripping in preexec is immune to PS1 changes by other hooks
#   - Double-source safe: guard prevents duplicate hook registration

if [[ -n "$__ZED_OSC133_INSTALLED" ]]; then
  return 0
fi
__ZED_OSC133_INSTALLED=1

# 0: no marks written yet
# 1: last C not yet closed with D
# 2: normal (D has been written or no command has run)
typeset -gi __zed_semantic_state=0

__zed_semantic_precmd() {
  local -i ret=$?
  builtin emulate -L zsh -o no_warn_create_global -o no_aliases

  if ! builtin zle 2>/dev/null; then
    if (( __zed_semantic_state == 1 )); then
      builtin printf '\e]133;D;%s\a' $ret
      (( __zed_semantic_state = 2 ))
    elif (( __zed_semantic_state == 2 )); then
      builtin printf '\e]133;D\a'
    fi
  fi

  local mark_a=$'%{\e]133;A\a%}'
  local mark_b=$'%{\e]133;B\a%}'

  if [[ -o prompt_percent ]]; then
    # Add A at the start and B at the end of PS1, but only if not already
    # present. This is the core of the pattern-match-and-strip approach:
    # we check before inserting so marks never double up.
    [[ $PS1 == *$mark_a* ]] || PS1=${mark_a}${PS1}
    [[ $PS1 == *$mark_b* ]] || PS1=${PS1}${mark_b}

    # Handle multiline prompts: mark continuation lines with secondary
    # prompt markers so the terminal can distinguish them.
    local mark_a_secondary=$'%{\e]133;A;k=s\a%}'
    if [[ $PS1 == ${mark_a}$'\n'* ]]; then
      local rest=${PS1#${mark_a}$'\n'}
      if [[ $rest == *$'\n'* ]]; then
        PS1=${mark_a}$'\n'${rest//$'\n'/$'\n'${mark_a_secondary}}
      fi
    elif [[ $PS1 == *$'\n'* ]]; then
      PS1=${PS1//$'\n'/$'\n'${mark_a_secondary}}
    fi

    # PS2 (continuation prompt) also gets markers.
    [[ $PS2 == *$mark_a_secondary* ]] || PS2=${mark_a_secondary}${PS2}
    [[ $PS2 == *$mark_b* ]] || PS2=${PS2}${mark_b}

    (( __zed_semantic_state = 2 ))
  elif ! builtin zle 2>/dev/null; then
    # prompt_percent is off — can't embed in PS1, fall back to printf.
    builtin printf '\e]133;A\a'
    (( __zed_semantic_state = 2 ))
  fi
}

__zed_semantic_preexec() {
  builtin emulate -L zsh -o no_warn_create_global -o no_aliases

  # Strip all marks from PS1/PS2 via pattern substitution. This is
  # unconditional — no equality check against a saved copy. Immune to
  # PS1 changes by other plugins between precmd and preexec.
  PS1=${PS1//$'%{\e]133;A\a%}'}
  PS1=${PS1//$'%{\e]133;A;k=s\a%}'}
  PS1=${PS1//$'%{\e]133;B\a%}'}
  PS2=${PS2//$'%{\e]133;A;k=s\a%}'}
  PS2=${PS2//$'%{\e]133;B\a%}'}

  builtin printf '\e]133;C\a'
  (( __zed_semantic_state = 1 ))
}

precmd_functions+=(__zed_semantic_precmd)
preexec_functions+=(__zed_semantic_preexec)