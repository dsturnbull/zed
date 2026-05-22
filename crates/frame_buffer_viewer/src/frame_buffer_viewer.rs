use std::sync::Arc;
use std::time::Duration;

use extension::frame_buffer::SharedFrameBuffer;
use gpui::{
    actions, div, img, px, App, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement, Pixels, RenderImage, Render, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, Styled, Task, Window,
};
use ui::prelude::*;
use workspace::item::{Item, ItemEvent, TabContentParams};
use workspace::{Pane, Workspace};

const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 20.0;
const ZOOM_STEP: f32 = 1.1;
const SCROLL_LINE_MULTIPLIER: f32 = 20.0;
const SCROLL_ZOOM_SENSITIVITY: f32 = 0.01;

actions!(frame_buffer_viewer, [ZoomIn, ZoomOut, ZoomReset]);

pub struct FrameBufferViewer {
    shared: Arc<SharedFrameBuffer>,
    zoom_level: f32,
    scroll_handle: ScrollHandle,
    pan_x: Pixels,
    focus_handle: FocusHandle,
    last_generation: u64,
    current_image: Option<Arc<RenderImage>>,
    _poll_task: Task<()>,
}

impl FrameBufferViewer {
    pub fn new(shared: Arc<SharedFrameBuffer>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let last_generation = shared.generation();
        let current_image = shared.current_frame();

        let _poll_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let should_notify = this.update(cx, |this, _cx| {
                    let generation = this.shared.generation();
                    if generation != this.last_generation {
                        this.last_generation = generation;
                        this.current_image = this.shared.current_frame();
                        true
                    } else {
                        false
                    }
                });
                match should_notify {
                    Ok(true) => {
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        });

        Self {
            shared,
            zoom_level: 1.0,
            scroll_handle: ScrollHandle::new(),
            pan_x: px(0.0),
            focus_handle,
            last_generation,
            current_image,
            _poll_task,
        }
    }

    fn set_zoom(&mut self, new_zoom: f32, cx: &mut Context<Self>) {
        let old_zoom = self.zoom_level;
        self.zoom_level = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (self.zoom_level - old_zoom).abs() > f32::EPSILON {
            if old_zoom > 0.0 {
                let ratio = self.zoom_level / old_zoom;
                self.pan_x *= ratio;
            }
            cx.notify();
        }
    }

    fn zoom_in(&mut self, _: &ZoomIn, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom_level * ZOOM_STEP, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom_level / ZOOM_STEP, cx);
    }

    fn zoom_reset(&mut self, _: &ZoomReset, _window: &mut Window, cx: &mut Context<Self>) {
        self.pan_x = px(0.0);
        self.set_zoom(1.0, cx);
    }

    fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.modifiers.control || event.modifiers.platform {
            let delta: f32 = match event.delta {
                ScrollDelta::Pixels(pixels) => pixels.y.into(),
                ScrollDelta::Lines(lines) => lines.y * SCROLL_LINE_MULTIPLIER,
            };
            let zoom_factor = if delta > 0.0 {
                1.0 + delta.abs() * SCROLL_ZOOM_SENSITIVITY
            } else {
                1.0 / (1.0 + delta.abs() * SCROLL_ZOOM_SENSITIVITY)
            };
            self.set_zoom(self.zoom_level * zoom_factor, cx);
        } else {
            let delta_x = match event.delta {
                ScrollDelta::Pixels(pixels) => pixels.x,
                ScrollDelta::Lines(lines) => px(lines.x * SCROLL_LINE_MULTIPLIER),
            };
            if delta_x != px(0.0) {
                self.pan_x += delta_x;
            }
            cx.notify();
        }
    }
}

impl EventEmitter<ItemEvent> for FrameBufferViewer {}

impl Focusable for FrameBufferViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FrameBufferViewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.shared.width;
        let height = self.shared.height;
        let zoom_percent = (self.zoom_level * 100.0).round() as u32;

        let status_text = format!("{}×{}  ·  {}%", width, height, zoom_percent);

        let status_bar = h_flex()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(Label::new(status_text).size(LabelSize::Small));

        let content = if let Some(image) = &self.current_image {
            let display_width = px(width as f32 * self.zoom_level);
            let display_height = px(height as f32 * self.zoom_level);

            let image_element = img(image.clone()).w(display_width).h(display_height);

            if self.pan_x != px(0.0) {
                div()
                    .relative()
                    .left(self.pan_x)
                    .child(div().items_center().child(image_element))
                    .into_any_element()
            } else {
                div()
                    .items_center()
                    .child(image_element)
                    .into_any_element()
            }
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Label::new("No frame presented yet")
                        .size(LabelSize::Large)
                        .color(Color::Muted),
                )
                .into_any_element()
        };

        v_flex()
            .id("FrameBufferViewer")
            .key_context("FrameBufferViewer")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .child(status_bar)
            .child(
                div()
                    .id("frame-buffer-scroll-container")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .on_scroll_wheel(cx.listener(Self::handle_scroll_wheel))
                    .child(content),
            )
    }
}

impl Item for FrameBufferViewer {
    type Event = ItemEvent;

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("Frame Buffer {}", self.shared.id.0).into()
    }

    fn tab_content(
        &self,
        params: TabContentParams,
        _window: &Window,
        cx: &App,
    ) -> gpui::AnyElement {
        let text = self.tab_content_text(params.detail.unwrap_or_default(), cx);
        Label::new(text)
            .single_line()
            .color(if params.selected {
                Color::Default
            } else {
                Color::Muted
            })
            .into_any_element()
    }
}

pub fn init(_cx: &mut App) {}

pub fn open_frame_buffer_viewer(
    shared: Arc<SharedFrameBuffer>,
    workspace: &mut Workspace,
    pane: Entity<Pane>,
    window: &mut Window,
    cx: &mut App,
) {
    let viewer = cx.new(|cx| FrameBufferViewer::new(shared, cx));
    workspace.add_item(
        pane,
        Box::new(viewer),
        None,
        true,
        true,
        window,
        cx,
    );
}