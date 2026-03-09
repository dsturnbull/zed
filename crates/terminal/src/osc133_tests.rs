//! End-to-end tests for OSC 133 shell integration.
//!
//! Spawn a real zsh with our shell integration hooks, run commands through
//! the PTY, and verify semantic zones (Prompt, Input, Output) are correct.

#[cfg(test)]
mod tests {
    use crate::{AlternateScroll, CursorShape, PathStyle, Terminal, TerminalBuilder};
    use alacritty_terminal::term::SemanticZoneType;
    use collections::HashMap;
    use gpui::{AppContext, TestAppContext};
    use std::time::Duration;
    use task::Shell;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme::init(theme::LoadThemes::JustBase, cx);
        });
    }

    async fn build_zsh_terminal(cx: &mut TestAppContext) -> gpui::Entity<Terminal> {
        let tmpdir =
            std::env::temp_dir().join(format!("zed-osc133-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmpdir);

        let mut env: HashMap<String, String> = HashMap::default();
        env.insert("HOME".into(), tmpdir.to_string_lossy().into_owned());
        env.insert("ZDOTDIR".into(), tmpdir.to_string_lossy().into_owned());
        env.insert("ZSH".into(), String::new());
        env.insert("FPATH".into(), String::new());

        let shell = Shell::Program("/bin/zsh".into());
        crate::setup_shell_integration(&shell, &mut env);

        let builder = cx
            .update(|cx| {
                TerminalBuilder::new(
                    Some(tmpdir.clone()),
                    None,
                    task::Shell::Program("/bin/zsh".into()),
                    env,
                    CursorShape::default(),
                    AlternateScroll::On,
                    None,
                    vec![],
                    0,
                    false,
                    0,
                    None,
                    cx,
                    vec![],
                    PathStyle::local(),
                )
            })
            .await
            .expect("Failed to create terminal builder");

        let terminal = cx.new(|cx| builder.subscribe(cx));
        // Real wall-clock sleep — the shell starts on an OS thread.
        std::thread::sleep(Duration::from_millis(500));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        terminal
    }

    fn send_input(terminal: &gpui::Entity<Terminal>, text: &str, cx: &mut TestAppContext) {
        terminal.update(cx, |terminal, _cx| {
            terminal.input(std::borrow::Cow::Owned(text.as_bytes().to_vec()));
        });
    }

    fn wait(cx: &mut TestAppContext) {
        // Real wall-clock sleep needed because the PTY event loop runs
        // on an OS thread, not the deterministic executor.
        std::thread::sleep(Duration::from_millis(200));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
    }

    fn zones(
        terminal: &gpui::Entity<Terminal>,
        cx: &mut TestAppContext,
    ) -> Vec<(SemanticZoneType, String)> {
        terminal.update(cx, |terminal, _cx| {
            let term = terminal.term.lock();
            term.semantic_zones()
                .iter()
                .map(|z| {
                    let text = term.bounds_to_string(z.start, z.end);
                    (z.zone_type, text.trim().to_string())
                })
                .collect()
        })
    }

    fn marks(terminal: &gpui::Entity<Terminal>, cx: &mut TestAppContext) -> Vec<String> {
        terminal.update(cx, |terminal, _cx| {
            let term = terminal.term.lock();
            term.semantic_marks()
                .iter()
                .map(|m| {
                    format!(
                        "{:?} at ({},{})",
                        m.mark_type, m.point.line.0, m.point.column.0
                    )
                })
                .collect()
        })
    }

    fn recent_outputs(
        terminal: &gpui::Entity<Terminal>,
        limit: usize,
        cx: &mut TestAppContext,
    ) -> Vec<(String, String, String)> {
        terminal.update(cx, |terminal, _cx| terminal.recent_command_outputs(limit))
    }

    #[gpui::test]
    async fn test_osc133_marks_received(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        init_test(cx);
        let terminal = build_zsh_terminal(cx).await;
        let m = marks(&terminal, cx);
        assert!(m.len() >= 2, "Expected >= 2 marks, got {}: {:?}", m.len(), m);
        assert!(m.iter().any(|s| s.contains("PromptStart")), "No PromptStart: {:?}", m);
    }

    #[gpui::test]
    async fn test_osc133_single_command(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        init_test(cx);
        let terminal = build_zsh_terminal(cx).await;
        send_input(&terminal, "echo hello-osc133-test
", cx);
        wait(cx);
        let z = zones(&terminal, cx);
        let types: Vec<_> = z.iter().map(|x| x.0).collect();
        assert!(types.contains(&SemanticZoneType::Prompt), "No Prompt: {:?}", z);
        assert!(types.contains(&SemanticZoneType::Input), "No Input: {:?}", z);
        assert!(types.contains(&SemanticZoneType::Output), "No Output: {:?}", z);
    }

    #[gpui::test]
    async fn test_osc133_zone_ordering(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        init_test(cx);
        let terminal = build_zsh_terminal(cx).await;
        send_input(&terminal, "echo ordering-test
", cx);
        wait(cx);
        let z = zones(&terminal, cx);
        let types: Vec<_> = z.iter().map(|x| x.0).collect();
        let has_cycle = types.windows(3).any(|w| {
            w == [SemanticZoneType::Prompt, SemanticZoneType::Input, SemanticZoneType::Output]
        });
        assert!(has_cycle, "No Prompt->Input->Output cycle in: {:?}", types);
    }

    #[gpui::test]
    async fn test_osc133_five_commands(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        init_test(cx);
        let terminal = build_zsh_terminal(cx).await;
        for i in 0..5 {
            send_input(&terminal, &format!("echo test-{}
", i), cx);
            wait(cx);
        }
        let z = zones(&terminal, cx);
        let m = marks(&terminal, cx);
        let out = z.iter().filter(|x| x.0 == SemanticZoneType::Output).count();
        let prompts = z.iter().filter(|x| x.0 == SemanticZoneType::Prompt).count();
        assert_eq!(out, 5, "Expected 5 Output zones, got {}. Marks: {:?}", out, m);
        assert!(prompts >= 5, "Expected >= 5 Prompt zones, got {}. Marks: {:?}", prompts, m);
    }

    #[gpui::test]
    async fn test_osc133_recent_outputs(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        init_test(cx);
        let terminal = build_zsh_terminal(cx).await;
        send_input(&terminal, "echo first-cmd
", cx);
        wait(cx);
        send_input(&terminal, "echo second-cmd
", cx);
        wait(cx);
        let out = recent_outputs(&terminal, 5, cx);
        assert!(out.len() >= 2, "Expected >= 2 outputs, got {}: {:?}", out.len(), out);
        assert!(out[0].2.contains("second-cmd"), "Most recent: {:?}", out[0]);
        assert!(out[1].2.contains("first-cmd"), "Second: {:?}", out[1]);
    }
}
