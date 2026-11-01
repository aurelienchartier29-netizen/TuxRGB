//! Motion wrappers used across the interface so nothing changes abruptly.
//!
//! * [`Veil`] – draws its content scaled / shifted (a transformation: no
//!   re-layout, so pages with canvases stay smooth) under a veil of a solid
//!   colour, and can ignore input while it animates. Page transitions (M3
//!   "fade through") and content swaps are built on it.
//! * [`swap`] – fades new content in (veil + slight lift) whenever its key
//!   changes: the effect settings panel, the device editor…
//! * [`reveal`] – expands / collapses its content (height spring, clipped)
//!   while fading it with a veil: inline sub-menus such as the colour
//!   picker, problem banners. Reports when it is fully hidden.
//! * [`smooth_scroll`] – wheel scrolling glides instead of jumping by
//!   lines, and the scrollbar thumb fades in when the pointer comes over the
//!   list instead of switching colour.
//! * [`text_field`] – the border of a text input eases into its hover and
//!   focus colours.
//!
//! Like the controls of [`super::motion`], every animation runs inside the
//! widget, frame by frame, only while it moves.

use crate::theme::Roles;
use crate::theme::tokens::{SpringSpec, motion};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::widget::{Operation, Widget};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::{
    Background, Color, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme,
    Transformation, Vector, mouse, window,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Critically damped follower used by the wrappers (same physics as
/// `theme::motion::Spring`, inlined per widget state).
#[derive(Clone, Copy, Debug)]
struct Follow {
    value: f32,
    velocity: f32,
    target: f32,
}

impl Follow {
    fn new(v: f32) -> Self {
        Self {
            value: v,
            velocity: 0.0,
            target: v,
        }
    }

    fn moving(&self) -> bool {
        (self.value - self.target).abs() > 0.001 || self.velocity.abs() > 0.01
    }

    fn step(&mut self, dt: f32, spec: SpringSpec) {
        if !self.moving() {
            self.value = self.target;
            self.velocity = 0.0;
            return;
        }
        let k = spec.stiffness;
        let c = 2.0 * spec.damping * k.sqrt();
        let mut remaining = dt.clamp(0.0, 0.15);
        const H: f32 = 1.0 / 480.0;
        while remaining > 0.0 {
            let h = remaining.min(H);
            let a = -k * (self.value - self.target) - c * self.velocity;
            self.velocity += a * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        if !self.moving() {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }
}

/// Animation time since the previous frame of a widget (at the animation
/// speed chosen in the settings).
fn frame_dt(last: &mut Option<Instant>, now: Instant) -> f32 {
    let dt = last
        .map(|t| now.saturating_duration_since(t).as_secs_f32())
        .unwrap_or(1.0 / 60.0)
        .min(0.05);
    *last = Some(now);
    crate::theme::motion::scaled(dt)
}

/// Smooth start and end.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// M3 "emphasized decelerate"-like curve: fast start, long gentle end.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// M3 "emphasized accelerate"-like curve: gentle start, fast end.
pub fn ease_in(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(2)
}

/// Transformation scaling `content` around the centre of `bounds`, then
/// shifting it by `offset`.
fn transform(bounds: Rectangle, scale: f32, offset: Vector) -> Transformation {
    let c = bounds.center();
    Transformation::translate(c.x + offset.x, c.y + offset.y)
        * Transformation::scale(scale)
        * Transformation::translate(-c.x, -c.y)
}

fn veil_quad(renderer: &mut Renderer, bounds: Rectangle, color: Color) {
    use iced::advanced::Renderer as _;
    if color.a <= 0.002 {
        return;
    }
    // A layer of its own: drawn after every nested layer of the content
    // (scrollables, stacked canvases), so it really covers everything.
    renderer.with_layer(bounds, |renderer| {
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..renderer::Quad::default()
            },
            Background::Color(color),
        );
    });
}

// ---------------------------------------------------------------------------
// Veil
// ---------------------------------------------------------------------------

/// Content drawn scaled / shifted under a coloured veil (see the module
/// documentation). Driven from outside (the app animates the values).
pub struct Veil<'a, Message> {
    content: Element<'a, Message>,
    veil: Color,
    scale: f32,
    offset: Vector,
    block: bool,
}

pub fn veil<'a, Message>(content: impl Into<Element<'a, Message>>) -> Veil<'a, Message> {
    Veil {
        content: content.into(),
        veil: Color::TRANSPARENT,
        scale: 1.0,
        offset: Vector::ZERO,
        block: false,
    }
}

impl<'a, Message> Veil<'a, Message> {
    /// Colour drawn over the content (its alpha is the veil amount).
    pub fn color(mut self, color: Color) -> Self {
        self.veil = color;
        self
    }

    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    pub fn offset(mut self, offset: Vector) -> Self {
        self.offset = offset;
        self
    }

    /// Ignore the mouse and keyboard (content in motion).
    pub fn block(mut self, block: bool) -> Self {
        self.block = block;
        self
    }

    fn transformed(&self) -> bool {
        (self.scale - 1.0).abs() > 1e-4 || self.offset != Vector::ZERO
    }
}

impl<'a, Message: 'a> Widget<Message, Theme, Renderer> for Veil<'a, Message> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Redraw events always go through: the content keeps animating.
        let redraw = matches!(event, Event::Window(window::Event::RedrawRequested(_)));
        if self.block && !redraw {
            return;
        }
        let cursor = if self.block || self.transformed() {
            mouse::Cursor::Unavailable
        } else {
            cursor
        };
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let cursor = if self.block {
            mouse::Cursor::Unavailable
        } else {
            cursor
        };
        if self.transformed() {
            let t = transform(bounds, self.scale, self.offset);
            renderer.with_layer(*viewport, |renderer| {
                renderer.with_transformation(t, |renderer| {
                    self.content.as_widget().draw(
                        tree,
                        renderer,
                        theme,
                        style,
                        layout,
                        mouse::Cursor::Unavailable,
                        &Rectangle::INFINITE,
                    );
                });
            });
        } else {
            self.content
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        }
        veil_quad(renderer, bounds, self.veil);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if self.block {
            return mouse::Interaction::default();
        }
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        if self.block {
            return None;
        }
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a> From<Veil<'a, Message>> for Element<'a, Message> {
    fn from(v: Veil<'a, Message>) -> Self {
        Element::new(v)
    }
}

// ---------------------------------------------------------------------------
// Swap: fade new content in when its key changes
// ---------------------------------------------------------------------------

/// Fades the content in (veil of `backdrop` + a small lift) whenever `key`
/// changes, e.g. another effect in the settings panel.
pub struct Swap<'a, Message> {
    content: Element<'a, Message>,
    key: u64,
    backdrop: Color,
}

pub fn swap<'a, Message>(
    key: u64,
    backdrop: Color,
    content: impl Into<Element<'a, Message>>,
) -> Swap<'a, Message> {
    Swap {
        content: content.into(),
        key,
        backdrop,
    }
}

struct SwapState {
    key: u64,
    /// 1 = just swapped (fully veiled), 0 = settled.
    fresh: Follow,
    last: Option<Instant>,
}

/// Lift of new content, in pixels, at the start of a swap.
const SWAP_LIFT: f32 = 10.0;
const SWAP_SPEC: SpringSpec = motion::DEFAULT_EFFECTS;

impl<'a, Message: 'a> Widget<Message, Theme, Renderer> for Swap<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SwapState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SwapState {
            key: self.key,
            fresh: Follow::new(0.0),
            last: None,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<SwapState>();
        if state.key != self.key {
            state.key = self.key;
            state.fresh = Follow {
                value: 1.0,
                velocity: 0.0,
                target: 0.0,
            };
            state.last = None;
        }
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        {
            let state = tree.state.downcast_mut::<SwapState>();
            if let Event::Window(window::Event::RedrawRequested(now)) = event
                && state.fresh.moving()
            {
                let dt = frame_dt(&mut state.last, *now);
                state.fresh.step(dt, SWAP_SPEC);
                shell.request_redraw();
            }
            if !state.fresh.moving() {
                state.last = None;
            }
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.children().next().unwrap(),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_ref::<SwapState>();
        let child = layout.children().next().unwrap();
        let f = state.fresh.value.clamp(0.0, 1.0);
        if f <= 0.001 {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child,
                cursor,
                viewport,
            );
            return;
        }
        let bounds = layout.bounds();
        // Small elements (a status chip) move less than a whole panel.
        let lift = Vector::new(0.0, SWAP_LIFT.min(bounds.height * 0.25) * ease_in_out(f));
        // Clipped to the element: the new content rises into its box and
        // nothing spills over the neighbours.
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            renderer.with_translation(lift, |renderer| {
                self.content.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    child,
                    cursor,
                    viewport,
                );
            });
        });
        // The new content emerges from the panel colour.
        let veil = Color {
            a: self.backdrop.a * f.powf(1.4),
            ..self.backdrop
        };
        veil_quad(renderer, clip, veil);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: 'a> From<Swap<'a, Message>> for Element<'a, Message> {
    fn from(s: Swap<'a, Message>) -> Self {
        Element::new(s)
    }
}

// ---------------------------------------------------------------------------
// Reveal: expand / collapse with a fade
// ---------------------------------------------------------------------------

/// Expands to the natural height of its content when `open`, collapses to
/// nothing otherwise, fading through `backdrop` (see the module docs).
pub struct Reveal<'a, Message> {
    content: Element<'a, Message>,
    open: bool,
    appear: bool,
    backdrop: Color,
    on_hidden: Option<Message>,
}

pub fn reveal<'a, Message>(
    open: bool,
    backdrop: Color,
    content: impl Into<Element<'a, Message>>,
) -> Reveal<'a, Message> {
    Reveal {
        content: content.into(),
        open,
        appear: true,
        backdrop,
        on_hidden: None,
    }
}

impl<'a, Message> Reveal<'a, Message> {
    /// Animate the first appearance (default) or start already open.
    pub fn appear(mut self, appear: bool) -> Self {
        self.appear = appear;
        self
    }

    /// Published once the content is fully collapsed.
    pub fn on_hidden(mut self, message: Message) -> Self {
        self.on_hidden = Some(message);
        self
    }
}

struct RevealState {
    t: Follow,
    last: Option<Instant>,
    hidden_sent: bool,
}

const REVEAL_SPEC: SpringSpec = motion::STANDARD_DEFAULT_SPATIAL;

impl<'a, Message: Clone + 'a> Widget<Message, Theme, Renderer> for Reveal<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<RevealState>()
    }

    fn state(&self) -> tree::State {
        let target = if self.open { 1.0 } else { 0.0 };
        let start = if self.appear { 0.0 } else { target };
        tree::State::new(RevealState {
            t: Follow {
                value: start,
                velocity: 0.0,
                target,
            },
            last: None,
            hidden_sent: false,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<RevealState>();
        let target = if self.open { 1.0 } else { 0.0 };
        if state.t.target != target {
            state.t.target = target;
            state.hidden_sent = false;
        }
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        let s = self.content.as_widget().size();
        Size::new(s.width, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let t = tree
            .state
            .downcast_ref::<RevealState>()
            .t
            .value
            .clamp(0.0, 1.0);
        let child = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        let size = child.size();
        layout::Node::with_children(
            Size::new(size.width, size.height * ease_in_out(t)),
            vec![child],
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let interactive = {
            let state = tree.state.downcast_mut::<RevealState>();
            if let Event::Window(window::Event::RedrawRequested(now)) = event {
                if state.t.moving() {
                    let dt = frame_dt(&mut state.last, *now);
                    state.t.step(dt, REVEAL_SPEC);
                    // The height changes: lay the page out again.
                    shell.invalidate_layout();
                    shell.request_redraw();
                } else {
                    state.last = None;
                    if state.t.target == 0.0 && !state.hidden_sent {
                        state.hidden_sent = true;
                        if let Some(m) = &self.on_hidden {
                            shell.publish(m.clone());
                        }
                    }
                }
            }
            state.t.target > 0.5 && state.t.value > 0.6
        };
        let redraw = matches!(event, Event::Window(window::Event::RedrawRequested(_)));
        if interactive || redraw {
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout.children().next().unwrap(),
                if interactive {
                    cursor
                } else {
                    mouse::Cursor::Unavailable
                },
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_ref::<RevealState>();
        let t = state.t.value.clamp(0.0, 1.0);
        let bounds = layout.bounds();
        if t <= 0.001 || bounds.height < 0.5 {
            return;
        }
        let child = layout.children().next().unwrap();
        if t >= 0.999 {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child,
                cursor,
                viewport,
            );
            return;
        }
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child,
                mouse::Cursor::Unavailable,
                &clip,
            );
        });
        let veil = Color {
            a: self.backdrop.a * (1.0 - t).powf(0.8),
            ..self.backdrop
        };
        veil_quad(renderer, clip, veil);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<RevealState>();
        if state.t.target < 0.5 {
            return mouse::Interaction::default();
        }
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }
}

impl<'a, Message: Clone + 'a> From<Reveal<'a, Message>> for Element<'a, Message> {
    fn from(r: Reveal<'a, Message>) -> Self {
        Element::new(r)
    }
}

// ---------------------------------------------------------------------------
// Smooth scrolling and a scrollbar that fades
// ---------------------------------------------------------------------------

/// Values shared between [`SmoothScroll`] and the style of its scrollable:
/// (pointer over the list 0..1, pointer over / dragging the thumb 0..1) and
/// whether the style saw the thumb hot during the last draw.
#[derive(Default)]
struct ScrollShared {
    over: Cell<f32>,
    hot: Cell<f32>,
    hot_now: Cell<bool>,
}

/// A vertical scrollable whose wheel scrolling glides and whose thumb fades
/// in and out. Use like `scrollable(...)`: `smooth_scroll(scrollable(c), r)`.
pub fn smooth_scroll<'a, Message: 'a>(
    scrollable: iced::widget::Scrollable<'a, Message>,
    r: &Roles,
) -> Element<'a, Message> {
    let shared = Rc::new(ScrollShared::default());
    let style = scroll_style(*r, shared.clone());
    Element::new(SmoothScroll {
        content: scrollable.style(style).into(),
        shared,
    })
}

fn scroll_style(
    r: Roles,
    shared: Rc<ScrollShared>,
) -> impl Fn(&Theme, iced::widget::scrollable::Status) -> iced::widget::scrollable::Style {
    use iced::widget::scrollable as s;
    move |theme, status| {
        let hot = matches!(
            status,
            s::Status::Hovered {
                is_vertical_scrollbar_hovered: true,
                ..
            } | s::Status::Dragged {
                is_vertical_scrollbar_dragged: true,
                ..
            }
        );
        shared.hot_now.set(hot);
        let mut st = s::default(theme, status);
        let a = 0.14 + 0.16 * shared.over.get() + 0.3 * shared.hot.get();
        let thumb = Color {
            a,
            ..r.on_surface_variant
        };
        for rail in [&mut st.vertical_rail, &mut st.horizontal_rail] {
            rail.background = None;
            rail.border = iced::Border::default();
            rail.scroller.background = Background::Color(thumb);
            rail.scroller.border = iced::Border {
                radius: crate::theme::tokens::shape::FULL.into(),
                ..iced::Border::default()
            };
        }
        st
    }
}

struct SmoothScroll<'a, Message> {
    content: Element<'a, Message>,
    shared: Rc<ScrollShared>,
}

#[derive(Default)]
struct SmoothState {
    /// Wheel distance still to scroll, in pixel-delta units.
    pending: Vector,
    /// Where the wheel turned (the glide keeps targeting that list).
    at: Option<Point>,
    shift: bool,
    over: f32,
    hot: f32,
    last: Option<Instant>,
}

/// Wheel glide time constant: a notch is ~95 % done in 0.18 s.
const GLIDE_TAU: f32 = 0.06;
/// Pixels per wheel line (iced's scrollable uses the same value).
const LINE: f32 = 60.0;

impl<'a, Message: 'a> Widget<Message, Theme, Renderer> for SmoothScroll<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SmoothState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SmoothState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let child_layout = layout.children().next().unwrap();
        let state = tree.state.downcast_mut::<SmoothState>();
        match event {
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                state.shift = m.shift();
            }
            // Mouse wheels move by whole lines: glide instead of jumping.
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x, y },
            }) if !state.shift && cursor.is_over(bounds) => {
                state.pending += Vector::new(*x, *y) * LINE;
                state.at = cursor.position();
                shell.capture_event();
                shell.request_redraw();
                return;
            }
            _ => {}
        }

        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let dt = frame_dt(&mut state.last, *now);
            let k = 1.0 - (-dt / 0.09).exp();
            let over_target = if cursor.is_over(bounds) { 1.0 } else { 0.0 };
            let hot_target = if self.shared.hot_now.get() { 1.0 } else { 0.0 };
            state.over += (over_target - state.over) * k;
            state.hot += (hot_target - state.hot) * k;
            if (state.over - over_target).abs() < 0.01 {
                state.over = over_target;
            }
            if (state.hot - hot_target).abs() < 0.01 {
                state.hot = hot_target;
            }
            let mut moving = state.over != over_target || state.hot != hot_target;

            if state.pending != Vector::ZERO {
                let g = 1.0 - (-dt / GLIDE_TAU).exp();
                let mut step = state.pending * g;
                if (state.pending.x.abs() + state.pending.y.abs()) < 1.0 {
                    step = state.pending;
                }
                state.pending -= step;
                if step != Vector::ZERO {
                    let glide = Event::Mouse(mouse::Event::WheelScrolled {
                        delta: mouse::ScrollDelta::Pixels {
                            x: step.x,
                            y: step.y,
                        },
                    });
                    let at = state.at.map(mouse::Cursor::Available).unwrap_or(cursor);
                    // Own shell: the list capturing the glide must not mark
                    // the redraw event as captured for the rest of the page.
                    let mut messages = Vec::new();
                    let mut inner = Shell::new(&mut messages);
                    self.content.as_widget_mut().update(
                        &mut tree.children[0],
                        &glide,
                        child_layout,
                        at,
                        renderer,
                        clipboard,
                        &mut inner,
                        viewport,
                    );
                    shell.merge(inner, |m| m);
                }
                moving = true;
            }
            if moving {
                shell.request_redraw();
            } else {
                state.last = None;
            }
        } else if matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. })) {
            // Pointer entering / leaving the list or the thumb: animate.
            shell.request_redraw();
        }

        let state = tree.state.downcast_ref::<SmoothState>();
        self.shared.over.set(state.over);
        self.shared.hot.set(state.hot);
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<SmoothState>();
        self.shared.over.set(state.over);
        self.shared.hot.set(state.hot);
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.children().next().unwrap(),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            viewport,
            translation,
        )
    }
}

// ---------------------------------------------------------------------------
// Text field: border eases into hover / focus
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FieldShared {
    hover: Cell<f32>,
    focus: Cell<f32>,
}

/// A text input whose border and fill ease into their hover and focus looks.
pub fn text_field<'a, Message: Clone + 'a>(
    input: iced::widget::TextInput<'a, Message>,
    r: &Roles,
) -> Element<'a, Message> {
    let shared = Rc::new(FieldShared::default());
    let style = field_style(*r, shared.clone());
    Element::new(TextField {
        content: input.style(style).into(),
        shared,
    })
}

fn field_style(
    r: Roles,
    shared: Rc<FieldShared>,
) -> impl Fn(&Theme, iced::widget::text_input::Status) -> iced::widget::text_input::Style {
    move |_, _| {
        let (h, f) = (shared.hover.get(), shared.focus.get());
        let mix = crate::theme::mix;
        let idle = r.outline_variant;
        let border = mix(mix(idle, r.on_surface_variant, h * 0.6), r.primary, f);
        iced::widget::text_input::Style {
            background: Background::Color(mix(
                r.surface_container_highest,
                r.surface_container_high,
                h * 0.5 * (1.0 - f),
            )),
            border: iced::Border {
                color: border,
                width: 1.0 + f,
                radius: crate::theme::tokens::shape::MEDIUM.into(),
            },
            icon: r.on_surface_variant,
            placeholder: Color {
                a: 0.7,
                ..r.on_surface_variant
            },
            value: r.on_surface,
            selection: Color {
                a: 0.35,
                ..r.primary
            },
        }
    }
}

struct TextField<'a, Message> {
    content: Element<'a, Message>,
    shared: Rc<FieldShared>,
}

#[derive(Default)]
struct FieldState {
    hover: f32,
    focus: f32,
    last: Option<Instant>,
}

type Paragraph = <Renderer as iced::advanced::text::Renderer>::Paragraph;

fn input_focused(tree: &Tree) -> bool {
    tree.state
        .downcast_ref::<iced::widget::text_input::State<Paragraph>>()
        .is_focused()
}

impl<'a, Message: 'a> Widget<Message, Theme, Renderer> for TextField<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<FieldState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(FieldState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.children().next().unwrap(),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        let focused = input_focused(&tree.children[0]);
        let over = cursor.is_over(layout.bounds());
        let state = tree.state.downcast_mut::<FieldState>();
        let (ht, ft) = (over as u8 as f32, focused as u8 as f32);
        match event {
            Event::Window(window::Event::RedrawRequested(now)) => {
                let dt = frame_dt(&mut state.last, *now);
                let k = 1.0 - (-dt / 0.06).exp();
                state.hover += (ht - state.hover) * k;
                state.focus += (ft - state.focus) * k;
                if (state.hover - ht).abs() < 0.01 {
                    state.hover = ht;
                }
                if (state.focus - ft).abs() < 0.01 {
                    state.focus = ft;
                }
                if state.hover != ht || state.focus != ft {
                    shell.request_redraw();
                } else {
                    state.last = None;
                }
            }
            _ if state.hover != ht || state.focus != ft => shell.request_redraw(),
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<FieldState>();
        self.shared.hover.set(state.hover);
        self.shared.focus.set(state.focus);
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.children().next().unwrap(),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_settles_without_overshoot_for_effects() {
        let mut f = Follow::new(1.0);
        f.target = 0.0;
        let mut min: f32 = 1.0;
        for _ in 0..120 {
            f.step(1.0 / 60.0, motion::DEFAULT_EFFECTS);
            min = min.min(f.value);
        }
        assert!(!f.moving());
        assert_eq!(f.value, 0.0);
        assert!(min >= -1e-3, "fades must not overshoot ({min})");
    }

    #[test]
    fn transform_keeps_the_centre_in_place() {
        let b = Rectangle::new(Point::new(100.0, 50.0), Size::new(200.0, 100.0));
        let t = transform(b, 0.5, Vector::ZERO);
        let c = b.center() * t;
        assert!((c.x - 200.0).abs() < 1e-3 && (c.y - 100.0).abs() < 1e-3);
        let corner = Point::new(100.0, 50.0) * t;
        assert!((corner.x - 150.0).abs() < 1e-3 && (corner.y - 75.0).abs() < 1e-3);
    }

    #[test]
    fn easings_are_monotonic_and_bounded() {
        for f in [ease_in_out, ease_out, ease_in] {
            let mut prev = f(0.0);
            assert_eq!(prev, 0.0);
            for i in 1..=100 {
                let v = f(i as f32 / 100.0);
                assert!(v >= prev - 1e-6);
                prev = v;
            }
            assert!((prev - 1.0).abs() < 1e-6);
        }
    }
}
