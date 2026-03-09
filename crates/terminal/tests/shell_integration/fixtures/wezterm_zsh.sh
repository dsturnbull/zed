# Wezterm zsh OSC 133 hooks — extracted from wezterm.sh
# Source: https://github.com/wezterm/wezterm/blob/main/assets/shell-integration/wezterm.sh
# License: MIT
#
# This is the semantic zone portion only, stripped of user vars, OSC 7, etc.

__wezterm_semantic_precmd_executing=""
__wezterm_semantic_precmd() {
  local ret="$?"
  if [[ "$__wezterm_semantic_precmd_executing" != "0" ]] ; then
    __wezterm_save_ps1="$PS1"
    __wezterm_save_ps2="$PS2"
    PS1=$'%{\e]133;P;k=i\a%}'$PS1$'%{\e]133;B\a%}'
    PS2=$'%{\e]133;P;k=s\a%}'$PS2$'%{\e]133;B\a%}'
    __wezterm_check_ps1="$PS1"
  fi
  if [[ "$__wezterm_semantic_precmd_executing" != "" ]] ; then
    printf "\033]133;D;%s\007" "$ret"
  fi
  printf "\033]133;A\007"
  __wezterm_semantic_precmd_executing=0
}

function __wezterm_semantic_preexec() {
  if [[ -n "${__wezterm_save_ps1+1}" && "${__wezterm_check_ps1-}" == "${PS1}" ]]; then
    PS1="$__wezterm_save_ps1"
    PS2="$__wezterm_save_ps2"
    unset __wezterm_save_ps1
  fi
  printf "\033]133;C;\007"
  __wezterm_semantic_precmd_executing=1
}

precmd_functions+=(__wezterm_semantic_precmd)
preexec_functions+=(__wezterm_semantic_preexec)