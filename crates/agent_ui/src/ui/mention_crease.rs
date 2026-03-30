use std::{ops::RangeInclusive, path::PathBuf, time::Duration};

use acp_thread::MentionUri;
use agent_client_protocol as acp;
use editor::{Anchor, Editor, SelectionEffects, ToOffset, scroll::Autoscroll};
use multi_buffer;
use std::ops::Range;
use gpui::{
    Animation, AnimationExt, AnyView, Context, IntoElement, WeakEntity, Window, pulsating_between,
};
use prompt_store::PromptId;
use rope::Point;
use settings::Settings;
use theme::ThemeSettings;
use ui::{ButtonLike, TintColor, Tooltip, prelude::*};
use workspace::{OpenOptions, Workspace};

#[derive(IntoElement)]
pub struct MentionCrease {
    id: ElementId,
    icon: SharedString,
    label: SharedString,
    mention_uri: Option<MentionUri>,
    workspace: Option<WeakEntity<Workspace>>,
    is_toggled: bool,
    is_loading: bool,
    tooltip: Option<SharedString>,
    image_preview: Option<Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>>,
    fold_range: Option<Range<Anchor>>,
    editor: Option<WeakEntity<Editor>>,
}

impl MentionCrease {
    pub fn new(
        id: impl Into<ElementId>,
        icon: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            icon: icon.into(),
            label: label.into(),
            mention_uri: None,
            workspace: None,
            is_toggled: false,
            is_loading: false,
            tooltip: None,
            image_preview: None,
            fold_range: None,
            editor: None,
        }
    }

    pub fn mention_uri(mut self, mention_uri: Option<MentionUri>) -> Self {
        self.mention_uri = mention_uri;
        self
    }

    pub fn workspace(mut self, workspace: Option<WeakEntity<Workspace>>) -> Self {
        self.workspace = workspace;
        self
    }

    pub fn is_toggled(mut self, is_toggled: bool) -> Self {
        self.is_toggled = is_toggled;
        self
    }

    pub fn is_loading(mut self, is_loading: bool) -> Self {
        self.is_loading = is_loading;
        self
    }

    pub fn fold_range(mut self, range: Range<Anchor>) -> Self {
        self.fold_range = Some(range);
        self
    }

    pub fn editor(mut self, editor: WeakEntity<Editor>) -> Self {
        self.editor = Some(editor);
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn image_preview(
        mut self,
        builder: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.image_preview = Some(Box::new(builder));
        self
    }
}

impl RenderOnce for MentionCrease {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let settings = ThemeSettings::get_global(cx);
        let font_size = settings.agent_buffer_font_size(cx);
        let buffer_font = settings.buffer_font.clone();
        let is_loading = self.is_loading;
        let tooltip = self.tooltip;
        let image_preview = self.image_preview;

        let button_height = DefiniteLength::Absolute(AbsoluteLength::Pixels(
            px(window.line_height().into()) - px(1.),
        ));

        ButtonLike::new(self.id)
            .style(ButtonStyle::Outlined)
            .size(ButtonSize::Compact)
            .height(button_height)
            .selected_style(ButtonStyle::Tinted(TintColor::Accent))
            .toggle_state(self.is_toggled)
            .map(|this| {
                let mention_uri = self.mention_uri.clone();
                let workspace = self.workspace.clone();
                let fold_range = self.fold_range.clone();
                let editor = self.editor.clone();

                if mention_uri.is_some() || fold_range.is_some() {
                    this.on_click(move |_event, window, cx| {
                        // Primary action: unfold to show content inline.
                        // This is consistent with how all other mention
                        // creases behave (file selections, diagnostics, etc).
                        if let Some((ref range, ref editor)) =
                            fold_range.clone().zip(editor.clone())
                        {
                            editor
                                .update(cx, |editor, cx| {
                                    editor.unfold_ranges(
                                        &[range.clone()],
                                        true,
                                        true,
                                        cx,
                                    );
                                })
                                .ok();
                            return;
                        }

                        // No fold_range: try navigation as fallback.
                        if let Some((ref uri, ref ws)) =
                            mention_uri.clone().zip(workspace.clone())
                        {
                            open_mention_uri(uri.clone(), ws, window, cx);
                        }
                    })
                } else {
                    this
                }
            })
            .child(
                h_flex()
                    .pb_px()
                    .gap_1()
                    .font(buffer_font)
                    .text_size(font_size)
                    .child(
                        Icon::from_path(self.icon.clone())
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(self.label.clone())
                    .map(|this| {
                        if is_loading {
                            this.with_animation(
                                "loading-context-crease",
                                Animation::new(Duration::from_secs(2))
                                    .repeat()
                                    .with_easing(pulsating_between(0.4, 0.8)),
                                |label, delta| label.opacity(delta),
                            )
                            .into_any()
                        } else {
                            this.into_any()
                        }
                    }),
            )
            .map(|button| {
                if let Some(image_preview) = image_preview {
                    button.hoverable_tooltip(image_preview)
                } else {
                    button.when_some(tooltip, |this, tooltip_text| {
                        this.tooltip(Tooltip::text(tooltip_text))
                    })
                }
            })
    }
}

/// Returns true if navigation succeeded, false if caller should fall back.
pub(crate) fn open_mention_uri(
    mention_uri: MentionUri,
    workspace: &WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(workspace) = workspace.upgrade() else {
        return false;
    };

    workspace.update(cx, |workspace, cx| match mention_uri {
        MentionUri::File { abs_path } => {
            open_file(workspace, abs_path, None, window, cx);
            true
        }
        MentionUri::Symbol {
            abs_path,
            line_range,
            ..
        }
        | MentionUri::Selection {
            abs_path: Some(abs_path),
            line_range,
        } => {
            open_file(workspace, abs_path, Some(line_range), window, cx);
            true
        }
        MentionUri::Directory { abs_path } => {
            reveal_in_project_panel(workspace, abs_path, cx);
            true
        }
        MentionUri::Thread { id, name } => {
            open_thread(workspace, id, name, window, cx);
            true
        }
        MentionUri::TextThread { .. } => true,
        MentionUri::Rule { id, .. } => {
            open_rule(workspace, id, window, cx);
            true
        }
        MentionUri::Fetch { url } => {
            cx.open_url(url.as_str());
            true
        }
        MentionUri::TerminalSelection {
            terminal_id,
            scroll_line,
            scroll_col,
            ..
        } => {
            return scroll_to_terminal_source(workspace, terminal_id, scroll_line, scroll_col, window, cx);
        }
        MentionUri::PastedImage
        | MentionUri::Selection { abs_path: None, .. }
        | MentionUri::Diagnostics { .. }
        | MentionUri::GitDiff { .. } => false,
    })
}

fn open_file(
    workspace: &mut Workspace,
    abs_path: PathBuf,
    line_range: Option<RangeInclusive<u32>>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let project = workspace.project();

    if let Some(project_path) =
        project.update(cx, |project, cx| project.find_project_path(&abs_path, cx))
    {
        let item = workspace.open_path(project_path, None, true, window, cx);
        if let Some(line_range) = line_range {
            window
                .spawn(cx, async move |cx| {
                    let Some(editor) = item.await?.downcast::<Editor>() else {
                        return Ok(());
                    };
                    editor
                        .update_in(cx, |editor, window, cx| {
                            let range = Point::new(*line_range.start(), 0)
                                ..Point::new(*line_range.start(), 0);
                            editor.change_selections(
                                SelectionEffects::scroll(Autoscroll::center()),
                                window,
                                cx,
                                |selections| selections.select_ranges(vec![range]),
                            );
                        })
                        .ok();
                    anyhow::Ok(())
                })
                .detach_and_log_err(cx);
        } else {
            item.detach_and_log_err(cx);
        }
    } else if abs_path.exists() {
        workspace
            .open_abs_path(
                abs_path,
                OpenOptions {
                    focus: Some(true),
                    ..Default::default()
                },
                window,
                cx,
            )
            .detach_and_log_err(cx);
    }
}

fn reveal_in_project_panel(
    workspace: &mut Workspace,
    abs_path: PathBuf,
    cx: &mut Context<Workspace>,
) {
    let project = workspace.project();
    let Some(entry_id) = project.update(cx, |project, cx| {
        let path = project.find_project_path(&abs_path, cx)?;
        project.entry_for_path(&path, cx).map(|entry| entry.id)
    }) else {
        return;
    };

    project.update(cx, |_, cx| {
        cx.emit(project::Event::RevealInProjectPanel(entry_id));
    });
}

fn open_thread(
    workspace: &mut Workspace,
    id: acp::SessionId,
    name: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    use crate::AgentPanel;

    let Some(panel) = workspace.panel::<AgentPanel>(cx) else {
        return;
    };

    panel.update(cx, |panel, cx| {
        panel.load_agent_thread(id, None, Some(name.into()), window, cx)
    });
}

fn scroll_to_terminal_source(
    workspace: &mut Workspace,
    terminal_id: Option<u64>,
    _scroll_line: Option<i32>,
    _scroll_col: Option<usize>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> bool {
    use terminal::alacritty_terminal::index::{Column, Line, Point as AlacPoint};
    use terminal_view::terminal_panel::TerminalPanel;

    let Some(id) = terminal_id else {
        log::info!("scroll_to_terminal_source: no terminal_id, falling back to unfold");
        return false;
    };
    log::info!("scroll_to_terminal_source: looking for terminal {id}");

    let Some(panel) = workspace.panel::<TerminalPanel>(cx) else {
        log::warn!("scroll_to_terminal_source: no TerminalPanel found");
        return false;
    };

    let Some((terminal_view, _)) = panel.read(cx).find_terminal_by_id(id, cx) else {
        log::info!("scroll_to_terminal_source: terminal {id} not found, falling back to unfold");
        return false;
    };
    log::info!("scroll_to_terminal_source: found terminal, focusing panel");

    // TODO: use semantic mark sequence number to find the exact zone.
    // For now, scroll to line -10 (10 lines into scrollback) as a proof
    // of concept to verify the focus + scroll + flash pipeline works.
    let target_line = Line(-10);
    let point = AlacPoint::new(target_line, Column(0));

    workspace.focus_panel::<TerminalPanel>(window, cx);
    terminal_view.update(cx, |view, cx| {
        log::info!(
            "scroll_to_terminal_source: scrolling to {:?} and setting flash",
            point,
        );
        view.prompt_flash = Some((target_line, std::time::Instant::now()));
        view.terminal().update(cx, |terminal, _| {
            terminal.scroll_to_point(point);
        });
        cx.notify();
    });
    true
}

fn open_rule(
    _workspace: &mut Workspace,
    id: PromptId,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    use zed_actions::assistant::OpenRulesLibrary;

    let PromptId::User { uuid } = id else {
        return;
    };

    window.dispatch_action(
        Box::new(OpenRulesLibrary {
            prompt_to_select: Some(uuid.0),
        }),
        cx,
    );
}
