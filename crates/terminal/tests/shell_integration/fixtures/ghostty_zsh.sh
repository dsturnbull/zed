# Ghostty zsh OSC 133 hooks — extracted from ghostty-integration
# Source: https://github.com/ghostty-org/ghostty/blob/main/src/shell-integration/zsh/ghostty-integration
# License: GPLv3
#
# This is the semantic zone portion only, stripped of sudo, ssh, cursor,
# title, cwd reporting, and zle hook installation complexity.
#
# Ghostty's approach: pattern-match to check if marks are already in PS1
# before adding, and strip marks in preexec via pattern substitution.
# This avoids the accumulation bug that affects save/restore approaches.

# 0: no OSC 133 [AC] marks have been written yet.
# 1: the last written OSC 133 C has not been closed with D yet.
# 2: none of the above.
typeset -gi _ghostty_state=0

# In a real terminal, ghostty opens a cloexec fd to /dev/tty.
# For testing, we just use stdout.
typeset -gi _ghostty_fd=1

_ghostty_precmd() {
    local -i cmd_status=$?
    emulate -L zsh -o no_warn_create_global -o no_aliases

    if ! zle 2>/dev/null; then
        if (( _ghostty_state == 1 )); then
            print -nu $_ghostty_fd '\e]133;D;'$cmd_status'\a'
            (( _ghostty_state = 2 ))
        elif (( _ghostty_state == 2 )); then
            print -nu $_ghostty_fd '\e]133;D\a'
        fi
    fi

    local mark1=$'%{\e]133;A;cl=line\a%}'
    if [[ -o prompt_percent ]]; then
        local mark2=$'%{\e]133;A;k=s\a%}'
        local markB=$'%{\e]133;B\a%}'
        [[ $PS1 == *$mark1* ]] || PS1=${mark1}${PS1}
        [[ $PS1 == *$markB* ]] || PS1=${PS1}${markB}

        if [[ $PS1 == ${mark1}$'\n'* ]]; then
            local rest=${PS1#${mark1}$'\n'}
            if [[ $rest == *$'\n'* ]]; then
                PS1=${mark1}$'\n'${rest//$'\n'/$'\n'${mark2}}
            fi
        elif [[ $PS1 == *$'\n'* ]]; then
            PS1=${PS1//$'\n'/$'\n'${mark2}}
        fi

        [[ $PS2 == *$mark2* ]] || PS2=${mark2}${PS2}
        [[ $PS2 == *$markB* ]] || PS2=${PS2}${markB}
        (( _ghostty_state = 2 ))
    elif ! zle 2>/dev/null; then
        print -rnu $_ghostty_fd -- ${mark1[3,-3]}
        (( _ghostty_state = 2 ))
    fi
}

_ghostty_preexec() {
    emulate -L zsh -o no_warn_create_global -o no_aliases

    # Strip marks from PS1/PS2 via pattern substitution — the key
    # difference from wezterm's save/restore approach.
    PS1=${PS1//$'%{\e]133;A;cl=line\a%}'}
    PS1=${PS1//$'%{\e]133;A;k=s\a%}'}
    PS1=${PS1//$'%{\e]133;B\a%}'}
    PS2=${PS2//$'%{\e]133;A;k=s\a%}'}
    PS2=${PS2//$'%{\e]133;B\a%}'}

    print -nu $_ghostty_fd '\e]133;C\a'
    (( _ghostty_state = 1 ))
}

precmd_functions+=(_ghostty_precmd)
preexec_functions+=(_ghostty_preexec)