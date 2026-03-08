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
        "zsh" | "bash" => Some(BASH_ZSH_OSC133_HOOKS),
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

/// Shell integration for OSC 133 semantic zones (bash + zsh).
///
/// Adapted from wezterm's shell-integration script which uses standard
/// OSC 133 sequences that work with any terminal. The script handles both
/// bash and zsh in a single file.
///
/// Source: <https://github.com/wezterm/wezterm/blob/main/assets/shell-integration/wezterm.sh>
/// License: MIT
///
/// The semantic zone sequences are:
///   A — prompt start (fresh line)
///   B — end of prompt, start of user input
///   C — end of user input, start of command output
///   D — end of command output (with exit status)
///
/// The precmd hook wraps PS1 with A at the start and B at the end,
/// restoring the original PS1 in preexec to avoid accumulation.
const BASH_ZSH_OSC133_HOOKS: &str = r#"
if [ -z "${BASH_VERSION-}" -a -z "${ZSH_NAME-}" ] ; then
  return 0 2>/dev/null || true
fi
if [[ $- != *i* ]] ; then
  return 0 2>/dev/null || true
fi

__zed_semantic_precmd_executing=""
__zed_semantic_precmd() {
  local ret="$?"
  if [[ "$__zed_semantic_precmd_executing" != "0" ]] ; then
    __zed_save_ps1="$PS1"
    __zed_save_ps2="$PS2"
    if [[ -n "$ZSH_NAME" ]] ; then
      PS1=$'%{\e]133;A\a%}'$PS1$'%{\e]133;B\a%}'
      PS2=$'%{\e]133;A;k=s\a%}'$PS2$'%{\e]133;B\a%}'
    else
      PS1='\[\e]133;A\a\]'$PS1'\[\e]133;B\a\]'
      PS2='\[\e]133;A;k=s\a\]'$PS2'\[\e]133;B\a\]'
    fi
    __zed_check_ps1="$PS1"
  fi
  if [[ "$__zed_semantic_precmd_executing" != "" ]] ; then
    printf "\033]133;D;%s\007" "$ret"
  fi
  printf "\033]133;A\007"
  __zed_semantic_precmd_executing=0
}

__zed_semantic_preexec() {
  if [[ -n "${__zed_save_ps1+1}" && "${__zed_check_ps1-}" == "${PS1}" ]]; then
    PS1="$__zed_save_ps1"
    PS2="$__zed_save_ps2"
    unset __zed_save_ps1
  fi
  printf "\033]133;C;\007"
  __zed_semantic_precmd_executing=1
}

precmd_functions+=(__zed_semantic_precmd)
preexec_functions+=(__zed_semantic_preexec)
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