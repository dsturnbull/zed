//! Shell integration for OSC 133 semantic zones.
//!
//! Writes shell-specific hook scripts to a cache directory and configures
//! environment variables so that bash, zsh, and fish emit OSC 133 escape
//! sequences marking prompt, input, and output boundaries.
//!
//! The bash/zsh hooks are adapted from wezterm's shell-integration script
//! which uses standard OSC 133 sequences compatible with any terminal:
//! <https://github.com/wezterm/wezterm/blob/main/assets/shell-integration/wezterm.sh>
//! (MIT license)
//!
//! The fish hooks follow the same pattern using fish's native event system.
//!
//! # Integration mechanism
//!
//! Rather than "typing" scripts into the PTY, each shell's native init
//! mechanism is used:
//!
//! - **zsh**: `ZDOTDIR` is redirected to a wrapper directory whose `.zshenv`
//!   restores the real `ZDOTDIR`, sources the user's `.zshenv`, then loads
//!   our hooks.
//! - **bash**: `--rcfile` points to a wrapper that sources `~/.bashrc` then
//!   our hooks. The extra args are returned for the caller to append.
//! - **fish**: `XDG_DATA_DIRS` is prepended with a directory containing a
//!   `fish/vendor_conf.d/` script.

use collections::HashMap;
use std::path::PathBuf;
use task::Shell;

/// Writes shell integration scripts to a cache directory and populates `env`
/// with the variables needed for each shell to source them at startup.
///
/// Returns extra shell arguments the caller should append (e.g. `--rcfile`
/// for bash). Returns an empty vec for shells that only need env vars.
pub fn setup(shell: &Shell, env: &mut HashMap<String, String>) -> Vec<String> {
    let shell_name = shell_basename(shell);
    match shell_name.as_str() {
        "zsh" => setup_zsh(env),
        "bash" => setup_bash(env),
        "fish" => setup_fish(env),
        _ => Vec::new(),
    }
}

fn shell_basename(shell: &Shell) -> String {
    match shell {
        Shell::System => std::env::var("SHELL")
            .ok()
            .and_then(|s| s.rsplit('/').next().map(String::from))
            .unwrap_or_default(),
        Shell::Program(program) => program
            .rsplit('/')
            .next()
            .unwrap_or(program)
            .to_string(),
        Shell::WithArguments { program, .. } => program
            .rsplit('/')
            .next()
            .unwrap_or(program)
            .to_string(),
    }
}

fn integration_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("zed")
        .join("shell-integration")
}

/// Version marker written alongside scripts so we can detect when the
/// embedded hooks have changed and need to be rewritten.
const HOOKS_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns true if the cached scripts are up-to-date.
fn is_current(subdir: &str) -> bool {
    let marker = integration_dir().join(subdir).join(".version");
    marker
        .exists()
        .then(|| std::fs::read_to_string(&marker).ok())
        .flatten()
        .is_some_and(|v| v.trim() == HOOKS_VERSION)
}

/// Mark the cached scripts as up-to-date.
fn stamp_version(subdir: &str) {
    let marker = integration_dir().join(subdir).join(".version");
    let _ = std::fs::write(marker, HOOKS_VERSION);
}

/// Remove all files in the shell-specific subdirectory then recreate it.
fn clean_dir(subdir: &str) -> Option<PathBuf> {
    let dir = integration_dir().join(subdir);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn write_script(subdir: &str, filename: &str, content: &str) -> Option<PathBuf> {
    let dir = integration_dir().join(subdir);
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(filename);
    std::fs::write(&path, content).ok()?;
    Some(path)
}

fn hooks_for_shell(shell_name: &str) -> Option<&'static str> {
    match shell_name {
        "zsh" => Some(ZSH_OSC133_HOOKS),
        "bash" => Some(BASH_OSC133_HOOKS),
        "fish" => Some(FISH_OSC133_HOOKS),
        _ => None,
    }
}

fn setup_zsh(env: &mut HashMap<String, String>) -> Vec<String> {
    if !is_current("zsh") {
        clean_dir("zsh");
    }

    let real_zdotdir = env
        .get("ZDOTDIR")
        .cloned()
        .or_else(|| std::env::var("ZDOTDIR").ok())
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });

    let hooks_content = hooks_for_shell("zsh").unwrap_or_default();

    let zshenv_content = format!(
        r#"# Zed shell integration — restore real ZDOTDIR then source hooks
export ZDOTDIR="{real_zdotdir}"
if [[ -f "$ZDOTDIR/.zshenv" ]]; then
  source "$ZDOTDIR/.zshenv"
fi
{hooks_content}
"#
    );

    let wrapper_dir = integration_dir().join("zsh");
    if std::fs::create_dir_all(&wrapper_dir).is_err() {
        return Vec::new();
    }
    if std::fs::write(wrapper_dir.join(".zshenv"), zshenv_content).is_err() {
        return Vec::new();
    }
    stamp_version("zsh");

    env.insert(
        "ZDOTDIR".to_string(),
        wrapper_dir.to_string_lossy().into_owned(),
    );
    Vec::new()
}

fn setup_bash(env: &mut HashMap<String, String>) -> Vec<String> {
    if !is_current("bash") {
        clean_dir("bash");
    }

    let hooks = hooks_for_shell("bash").unwrap_or_default();
    let rcfile_content = format!(
        r#"# Zed shell integration — source user's bashrc then hooks
if [[ -f ~/.bashrc ]]; then
  source ~/.bashrc
fi
{hooks}
"#
    );

    if let Some(path) = write_script("bash", "zed_bashrc", &rcfile_content) {
        stamp_version("bash");
        vec![
            "--rcfile".to_string(),
            path.to_string_lossy().into_owned(),
        ]
    } else {
        if let Some(path) = write_script("bash", "zed_hooks.bash", hooks) {
            env.insert("BASH_ENV".to_string(), path.to_string_lossy().into_owned());
        }
        Vec::new()
    }
}

fn setup_fish(env: &mut HashMap<String, String>) -> Vec<String> {
    if !is_current("fish") {
        clean_dir("fish");
    }

    let hooks = hooks_for_shell("fish").unwrap_or_default();
    let fish_vendor = integration_dir().join("fish");
    let conf_dir = fish_vendor.join("fish").join("vendor_conf.d");
    if std::fs::create_dir_all(&conf_dir).is_ok() {
        let _ = std::fs::write(conf_dir.join("zed_osc133.fish"), hooks);
        stamp_version("fish");
    }

    let existing = env
        .get("XDG_DATA_DIRS")
        .cloned()
        .or_else(|| std::env::var("XDG_DATA_DIRS").ok())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    env.insert(
        "XDG_DATA_DIRS".to_string(),
        format!("{}:{}", fish_vendor.to_string_lossy(), existing),
    );
    Vec::new()
}

// ---------------------------------------------------------------------------
// Hook scripts
// ---------------------------------------------------------------------------

/// Zsh shell integration for OSC 133 semantic zones.
///
/// Uses ghostty's pattern-match-and-strip strategy for PS1:
///   - precmd: emit D (if a command ran), then add A and B marks to PS1
///     using pattern matching (`[[ $PS1 == *$mark* ]]`) to prevent
///     double-insertion.
///   - preexec: strip all marks from PS1/PS2 via pattern substitution
///     (`PS1=${PS1//$mark}`), then emit C.
///
/// This gives us:
///   - No accumulation: pattern matching prevents double-wrapping regardless
///     of what other plugins do to PS1.
///   - Redraw survival: marks are embedded in PS1, so they're re-emitted on
///     window resize (SIGWINCH), `zle reset-prompt` (async plugins like
///     powerlevel10k/pure), and SIGCHLD with `notify` set.
///   - Plugin-safe: stripping in preexec is unconditional — no equality check
///     against a saved PS1 copy, so immune to PS1 changes by other hooks.
///   - Double-source safe: guard prevents duplicate hook registration.
///
/// The semantic zone sequences are:
///   A — prompt start
///   B — end of prompt / start of user input
///   C — end of user input / start of command output
///   D — end of command output with exit status
const ZSH_OSC133_HOOKS: &str = r#"
if [[ -z "$ZSH_NAME" ]] ; then
  return 0 2>/dev/null || true
fi
if [[ $- != *i* ]] ; then
  return 0 2>/dev/null || true
fi
if [[ -n "$__ZED_OSC133_INSTALLED" ]] ; then
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
    [[ $PS1 == *$mark_a* ]] || PS1=${mark_a}${PS1}
    [[ $PS1 == *$mark_b* ]] || PS1=${PS1}${mark_b}

    local mark_a_secondary=$'%{\e]133;A;k=s\a%}'
    if [[ $PS1 == ${mark_a}$'\n'* ]]; then
      local rest=${PS1#${mark_a}$'\n'}
      if [[ $rest == *$'\n'* ]]; then
        PS1=${mark_a}$'\n'${rest//$'\n'/$'\n'${mark_a_secondary}}
      fi
    elif [[ $PS1 == *$'\n'* ]]; then
      PS1=${PS1//$'\n'/$'\n'${mark_a_secondary}}
    fi

    [[ $PS2 == *$mark_a_secondary* ]] || PS2=${mark_a_secondary}${PS2}
    [[ $PS2 == *$mark_b* ]] || PS2=${PS2}${mark_b}

    (( __zed_semantic_state = 2 ))
  elif ! builtin zle 2>/dev/null; then
    builtin printf '\e]133;A\a'
    (( __zed_semantic_state = 2 ))
  fi
}

__zed_semantic_preexec() {
  builtin emulate -L zsh -o no_warn_create_global -o no_aliases

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
"#;

/// Bash shell integration for OSC 133 semantic zones.
///
/// Uses PROMPT_COMMAND for A/D marks, PS1 suffix for B, and PS0 for C
/// (bash 4.4+, falls back to DEBUG trap for older bash).
///
/// PS1 wrapping is still needed for bash (no zle-line-init equivalent),
/// but we only append B — never embed A — and use a sentinel to prevent
/// accumulation.
const BASH_OSC133_HOOKS: &str = r#"
if [[ -z "${BASH_VERSION-}" ]] ; then
  return 0 2>/dev/null || true
fi
if [[ $- != *i* ]] ; then
  return 0 2>/dev/null || true
fi
if [[ -n "$__ZED_OSC133_INSTALLED" ]] ; then
  return 0
fi
__ZED_OSC133_INSTALLED=1

__zed_semantic_prompt_command() {
  local ret="$?"
  if [[ -n "$__zed_semantic_executing" ]] ; then
    printf "\033]133;D;%s\007" "$ret"
  fi
  printf "\033]133;A\007"
  # Append B to PS1 only if our sentinel is not already present.
  if [[ "$PS1" != *$'\e]133;B\a'* ]] ; then
    PS1="$PS1"'\[\e]133;B\a\]'
  fi
  __zed_semantic_executing=""
}

# Use PS0 (bash 4.4+) to emit C after user input, before command runs.
if [[ "${BASH_VERSINFO[0]}" -ge 5 ]] || \
   [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 ]] ; then
  PS0='\[\e]133;C\a\]'
  __zed_semantic_preexec_via_debug() {
    __zed_semantic_executing=1
  }
  trap '__zed_semantic_preexec_via_debug' DEBUG
else
  trap 'printf "\033]133;C\007"; __zed_semantic_executing=1' DEBUG
fi

PROMPT_COMMAND="__zed_semantic_prompt_command${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
"#;

/// Fish shell integration for OSC 133 semantic zones.
///
/// Uses fish's native event system (`fish_prompt` and `fish_preexec`).
/// Fish 3.4+ may emit OSC 133 natively when `TERM_PROGRAM` is recognized,
/// but this ensures coverage for older versions and custom prompts.
const FISH_OSC133_HOOKS: &str = r#"
if not set -q _ZED_OSC133_INSTALLED
  set -g _ZED_OSC133_INSTALLED 1
  set -g __zed_semantic_state ""
  function __zed_semantic_precmd --on-event fish_prompt
    set -l ret $status
    if test -n "$__zed_semantic_state"
      printf '\e]133;D;%s\a' $ret
    end
    printf '\e]133;A\a'
    set -g __zed_semantic_state "prompt"
  end
  function __zed_semantic_preexec --on-event fish_preexec
    printf '\e]133;C\a'
    set -g __zed_semantic_state "exec"
  end
end
"#;