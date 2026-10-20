//! Animated M3 Expressive controls.
//!
//! * [`button`] – drop-in replacement for `iced::widget::button`: the state
//!   layer fades in on hover and the shape morphs (pill → squircle) on press
//!   with a spring that may overshoot slightly, like Material 3 Expressive
//!   buttons. Every existing button style works unchanged: the widget asks
//!   the style for its Active / Hovered / Pressed looks and blends them.
//!   When those looks change (a segment gets selected, a button becomes
//!   enabled, the theme changes) the button eases into the new ones instead
//!   of switching.
//! * [`switch`] – M3 switch whose thumb slides and grows with a spring,
//!   stretches in the direction it travels and shows a check mark when on.
//! * [`slider`] – M3 Expressive slider: the track thickens on hover and
//!   while dragging, the handle narrows when grabbed and squashes /
//!   stretches with the drag speed, the value follows the finger through a
//!   spring, the ends rubber-band when pulled past, stop indicators act as
//!   detents and a value bubble pops up above the handle.
//! * [`shape_badge`] – an icon on an expressive "cookie" (scalloped) shape.
//!
//! Animations run per widget, only while they move: a redraw is requested
//! frame by frame until the springs settle, then nothing.

use crate::theme::tokens::{SpringSpec, motion};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::widget::{Operation, Widget};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::widget::button::{Status, Style};
use iced::widget::canvas;
use iced::{
    Background, Border, Color, Element, Event, Length, Padding, Point, Rectangle, Renderer, Shadow,
    Size, Theme, Vector, mouse, touch, window,
};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Tiny spring (same physics as theme::motion::Spring, inlined per widget).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Anim {
    value: f32,
    velocity: f32,
    target: f32,
}

impl Anim {
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

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: lerp(a.r, b.r, t),
        g: lerp(a.g, b.g, t),
        b: lerp(a.b, b.b, t),
        a: lerp(a.a, b.a, t),
    }
}

fn bg_color(bg: Option<Background>, fallback: Color) -> Color {
    match bg {
        Some(Background::Color(c)) => c,
        _ => Color { a: 0.0, ..fallback },
    }
}

/// Corner radii larger than half the button are drawn as a pill anyway:
/// clamp them first so the morph is visible over the whole animation.
fn clamp_radius(mut s: Style, max: f32) -> Style {
    let r = &mut s.border.radius;
    r.top_left = r.top_left.min(max);
    r.top_right = r.top_right.min(max);
    r.bottom_right = r.bottom_right.min(max);
    r.bottom_left = r.bottom_left.min(max);
    s
}

/// Blends two button styles. Colours use `t` clamped to 0..=1, the corner
/// radius uses the raw spring value so the morph can overshoot a little.
fn blend(a: &Style, b: &Style, t: f32) -> Style {
    let tc = t.clamp(0.0, 1.0);
    let bga = bg_color(a.background, bg_color(b.background, Color::TRANSPARENT));
    let bgb = bg_color(b.background, bga);
    let radius = |ra: f32, rb: f32| lerp(ra, rb, t).max(0.0);
    Style {
        background: if a.background.is_none() && b.background.is_none() {
            None
        } else {
            Some(Background::Color(lerp_color(bga, bgb, tc)))
        },
        text_color: lerp_color(a.text_color, b.text_color, tc),
        border: Border {
            color: lerp_color(a.border.color, b.border.color, tc),
            width: lerp(a.border.width, b.border.width, tc),
            radius: iced::border::Radius {
                top_left: radius(a.border.radius.top_left, b.border.radius.top_left),
                top_right: radius(a.border.radius.top_right, b.border.radius.top_right),
                bottom_right: radius(a.border.radius.bottom_right, b.border.radius.bottom_right),
                bottom_left: radius(a.border.radius.bottom_left, b.border.radius.bottom_left),
            },
        },
        shadow: Shadow {
            color: lerp_color(a.shadow.color, b.shadow.color, tc),
            offset: Vector::new(
                lerp(a.shadow.offset.x, b.shadow.offset.x, tc),
                lerp(a.shadow.offset.y, b.shadow.offset.y, tc),
            ),
            blur_radius: lerp(a.shadow.blur_radius, b.shadow.blur_radius, tc),
        },
        snap: a.snap,
    }
}

// ---------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------

type StyleFn<'a> = Box<dyn Fn(&Theme, Status) -> Style + 'a>;

pub struct MotionButton<'a, Message> {
    content: Element<'a, Message>,
    on_press: Option<Message>,
    width: Length,
    height: Length,
    padding: Padding,
    style: StyleFn<'a>,
}

/// Animated button (see the module documentation).
pub fn button<'a, Message>(content: impl Into<Element<'a, Message>>) -> MotionButton<'a, Message> {
    let content = content.into();
    let size = content.as_widget().size_hint();
    MotionButton {
        content,
        on_press: None,
        width: size.width.fluid(),
        height: size.height.fluid(),
        padding: Padding::from([5.0, 10.0]),
        style: Box::new(iced::widget::button::primary),
    }
}

impl<'a, Message> MotionButton<'a, Message> {
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    pub fn on_press(mut self, message: Message) -> Self {
        self.on_press = Some(message);
        self
    }

    pub fn on_press_maybe(mut self, message: Option<Message>) -> Self {
        self.on_press = message;
        self
    }

    pub fn style(mut self, style: impl Fn(&Theme, Status) -> Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }

    /// What the style function asks for now: Active / Hovered / Pressed, or
    /// the disabled look three times.
    fn targets(&self, theme: &Theme, bounds: Rectangle) -> [Style; 3] {
        let max = bounds.width.min(bounds.height) / 2.0;
        let get = |status| clamp_radius((self.style)(theme, status), max);
        if self.on_press.is_none() {
            let d = get(Status::Disabled);
            [d, d, d]
        } else {
            [
                get(Status::Active),
                get(Status::Hovered),
                get(Status::Pressed),
            ]
        }
    }
}

struct ButtonState {
    pressed: bool,
    hover: Anim,
    press: Anim,
    last: Option<Instant>,
    /// Displayed Active / Hovered / Pressed looks, following what the style
    /// function returns with a short ease (see [`STYLE_TAU`]).
    shown: Option<[Style; 3]>,
    style_last: Option<Instant>,
    /// Theme of the last draw: `update` has none, the targets need it.
    theme: std::cell::RefCell<Option<Theme>>,
}

impl Default for ButtonState {
    fn default() -> Self {
        Self {
            pressed: false,
            hover: Anim::new(0.0),
            press: Anim::new(0.0),
            last: None,
            shown: None,
            style_last: None,
            theme: std::cell::RefCell::new(None),
        }
    }
}

/// Time constant of style changes (selection, enabled, theme): ~95 % in
/// 0.2 s, and a style that keeps changing (morph) is followed closely.
const STYLE_TAU: f32 = 0.07;

fn close_color(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 0.004
        && (a.g - b.g).abs() < 0.004
        && (a.b - b.b).abs() < 0.004
        && (a.a - b.a).abs() < 0.004
}

/// Two looks are visually identical (end of a style transition).
fn close_style(a: &Style, b: &Style) -> bool {
    let bg = |s: &Style| bg_color(s.background, Color::TRANSPARENT);
    let r = |s: &Style| {
        let r = s.border.radius;
        [r.top_left, r.top_right, r.bottom_right, r.bottom_left]
    };
    close_color(bg(a), bg(b))
        && close_color(a.text_color, b.text_color)
        && close_color(a.border.color, b.border.color)
        && close_color(a.shadow.color, b.shadow.color)
        && (a.border.width - b.border.width).abs() < 0.05
        && (a.shadow.blur_radius - b.shadow.blur_radius).abs() < 0.05
        && r(a).iter().zip(r(b)).all(|(x, y)| (x - y).abs() < 0.05)
}

/// Hover: colour only, never overshoots. Press: spatial, expressive bounce.
const HOVER_SPEC: SpringSpec = motion::FAST_EFFECTS;
const PRESS_SPEC: SpringSpec = SpringSpec {
    damping: 0.55,
    stiffness: 1100.0,
};

impl<'a, Message: Clone + 'a> Widget<Message, Theme, Renderer> for MotionButton<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ButtonState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ButtonState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::padded(limits, self.width, self.height, self.padding, |limits| {
            self.content
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits)
        })
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout.children().next().unwrap(),
                renderer,
                operation,
            );
        });
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
        if shell.is_event_captured() {
            return;
        }
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<ButtonState>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerPressed { .. }) => {
                if self.on_press.is_some() && cursor.is_over(bounds) {
                    state.pressed = true;
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerLifted { .. }) => {
                if let Some(message) = &self.on_press
                    && state.pressed
                {
                    state.pressed = false;
                    if cursor.is_over(bounds) {
                        shell.publish(message.clone());
                    }
                    shell.capture_event();
                }
            }
            Event::Touch(touch::Event::FingerLost { .. }) => state.pressed = false,
            _ => {}
        }

        let enabled = self.on_press.is_some();
        let over = enabled && cursor.is_over(bounds);
        state.hover.target = if over { 1.0 } else { 0.0 };
        state.press.target = if over && state.pressed { 1.0 } else { 0.0 };

        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let dt = crate::theme::motion::scaled(
                state
                    .last
                    .map(|t| now.saturating_duration_since(t).as_secs_f32())
                    .unwrap_or(1.0 / 60.0)
                    .min(0.05),
            );
            state.hover.step(dt, HOVER_SPEC);
            state.press.step(dt, PRESS_SPEC);
            if state.hover.moving() || state.press.moving() {
                state.last = Some(*now);
                shell.request_redraw();
            } else {
                state.last = None;
            }
            // Style transitions (selection, enabled state, theme).
            let targets = state
                .theme
                .borrow()
                .as_ref()
                .map(|theme| self.targets(theme, bounds));
            if let Some(targets) = targets {
                match &mut state.shown {
                    None => state.shown = Some(targets),
                    Some(shown) => {
                        let dt = crate::theme::motion::scaled(
                            state
                                .style_last
                                .map(|t| now.saturating_duration_since(t).as_secs_f32())
                                .unwrap_or(1.0 / 60.0)
                                .min(0.05),
                        );
                        let k = 1.0 - (-dt / STYLE_TAU).exp();
                        let mut moving = false;
                        for (s, t) in shown.iter_mut().zip(&targets) {
                            if close_style(s, t) {
                                *s = *t;
                            } else {
                                *s = blend(s, t, k);
                                moving = true;
                            }
                        }
                        if moving {
                            state.style_last = Some(*now);
                            shell.request_redraw();
                        } else {
                            state.style_last = None;
                        }
                    }
                }
            }
        } else if state.hover.moving() || state.press.moving() || state.style_last.is_some() {
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<ButtonState>();
        // Remember the theme for the style transitions computed in `update`.
        {
            let mut cell = state.theme.borrow_mut();
            if cell.as_ref() != Some(theme) {
                *cell = Some(theme.clone());
            }
        }
        let [active, hovered, pressed] = state.shown.unwrap_or_else(|| self.targets(theme, bounds));
        let style = if self.on_press.is_none() {
            active
        } else {
            let base = blend(&active, &hovered, state.hover.value);
            blend(&base, &pressed, state.press.value)
        };
        // Squish: pressed buttons compress (more horizontally than
        // vertically, like a soft object), then spring back with a small
        // overshoot; hovered buttons swell by a hair.
        let squish = state.press.value;
        let swell = state.hover.value.clamp(0.0, 1.0) * (1.0 - squish.clamp(0.0, 1.0));
        let dx = (squish * 0.035 * bounds.width).min(5.0) - swell * 1.0;
        let dy = (squish * 0.06 * bounds.height).min(2.5) - swell * 0.5;
        let quad = Rectangle {
            x: bounds.x + dx,
            y: bounds.y + dy,
            width: (bounds.width - 2.0 * dx).max(1.0),
            height: (bounds.height - 2.0 * dy).max(1.0),
        };
        if style.background.is_some() || style.border.width > 0.0 || style.shadow.color.a > 0.0 {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: quad,
                    border: style.border,
                    shadow: style.shadow,
                    snap: style.snap,
                },
                style
                    .background
                    .unwrap_or(Background::Color(Color::TRANSPARENT)),
            );
        }
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            &renderer::Style {
                text_color: style.text_color,
            },
            layout.children().next().unwrap(),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) && self.on_press.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
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

impl<'a, Message: Clone + 'a> From<MotionButton<'a, Message>> for Element<'a, Message> {
    fn from(b: MotionButton<'a, Message>) -> Self {
        Element::new(b)
    }
}

// ---------------------------------------------------------------------------
// Switch
// ---------------------------------------------------------------------------

/// Colours of the M3 switch.
#[derive(Clone, Copy, Debug)]
pub struct SwitchColors {
    pub track_on: Color,
    pub track_off: Color,
    pub outline_off: Color,
    pub thumb_on: Color,
    pub thumb_off: Color,
    pub icon_on: Color,
    pub state_layer: Color,
}

pub struct Switch<'a, Message> {
    on: bool,
    on_toggle: Option<Box<dyn Fn(bool) -> Message + 'a>>,
    colors: SwitchColors,
}

/// M3 switch: 52 × 32 track, thumb 16 dp off → 24 dp on → 28 dp pressed.
pub fn switch<'a, Message>(
    on: bool,
    colors: SwitchColors,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> Switch<'a, Message> {
    Switch {
        on,
        on_toggle: Some(Box::new(on_toggle)),
        colors,
    }
}

struct SwitchState {
    pos: Anim,
    press: Anim,
    hover: Anim,
    pressed: bool,
    last: Option<Instant>,
    initialised: bool,
}

const TRACK: Size = Size::new(52.0, 32.0);

impl<'a, Message: Clone + 'a> Widget<Message, Theme, Renderer> for Switch<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SwitchState>()
    }

    fn state(&self) -> tree::State {
        let v = if self.on { 1.0 } else { 0.0 };
        tree::State::new(SwitchState {
            pos: Anim::new(v),
            press: Anim::new(0.0),
            hover: Anim::new(0.0),
            pressed: false,
            last: None,
            initialised: true,
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(
            Length::Fixed(TRACK.width + 8.0),
            Length::Fixed(TRACK.height + 8.0),
        )
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(
            Length::Fixed(TRACK.width + 8.0),
            Length::Fixed(TRACK.height + 8.0),
            Size::new(TRACK.width + 8.0, TRACK.height + 8.0),
        ))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<SwitchState>();
        if !state.initialised {
            state.pos = Anim::new(if self.on { 1.0 } else { 0.0 });
            state.initialised = true;
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerPressed { .. })
                if self.on_toggle.is_some() && cursor.is_over(bounds) =>
            {
                state.pressed = true;
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerLifted { .. })
                if state.pressed =>
            {
                state.pressed = false;
                if cursor.is_over(bounds)
                    && let Some(f) = &self.on_toggle
                {
                    shell.publish(f(!self.on));
                }
                shell.capture_event();
            }
            _ => {}
        }
        let over = self.on_toggle.is_some() && cursor.is_over(bounds);
        state.pos.target = if self.on { 1.0 } else { 0.0 };
        state.press.target = if state.pressed { 1.0 } else { 0.0 };
        state.hover.target = if over { 1.0 } else { 0.0 };
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let dt = crate::theme::motion::scaled(
                state
                    .last
                    .map(|t| now.saturating_duration_since(t).as_secs_f32())
                    .unwrap_or(1.0 / 60.0)
                    .min(0.05),
            );
            state.pos.step(dt, motion::EXPRESSIVE_FAST_SPATIAL);
            state.press.step(dt, PRESS_SPEC);
            state.hover.step(dt, HOVER_SPEC);
            if state.pos.moving() || state.press.moving() || state.hover.moving() {
                state.last = Some(*now);
                shell.request_redraw();
            } else {
                state.last = None;
            }
        } else if state.pos.moving() || state.press.moving() || state.hover.moving() {
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        use iced::advanced::text::Renderer as _;
        let state = tree.state.downcast_ref::<SwitchState>();
        let c = &self.colors;
        let b = layout.bounds();
        let track = Rectangle::new(
            Point::new(
                b.x + (b.width - TRACK.width) / 2.0,
                b.y + (b.height - TRACK.height) / 2.0,
            ),
            TRACK,
        );
        let p = state.pos.value; // may overshoot a bit
        let pc = p.clamp(0.0, 1.0);
        // Track.
        renderer.fill_quad(
            renderer::Quad {
                bounds: track,
                border: Border {
                    color: lerp_color(c.outline_off, c.track_on, pc),
                    width: 2.0 * (1.0 - pc),
                    radius: (TRACK.height / 2.0).into(),
                },
                shadow: Shadow::default(),
                snap: false,
            },
            Background::Color(lerp_color(c.track_off, c.track_on, pc)),
        );
        // Thumb: 16 → 24 dp, 28 dp while pressed; while it travels it
        // stretches along the track and squashes across (volume kept).
        let d = lerp(16.0, 24.0, pc) + 4.0 * state.press.value.clamp(0.0, 1.2);
        let stretch = (state.pos.velocity.abs() / 9.0).clamp(0.0, 0.45);
        let (tw, th) = (d * (1.0 + stretch), d / (1.0 + stretch * 0.7));
        let cx_off = track.x + TRACK.height / 2.0;
        let cx_on = track.x + TRACK.width - TRACK.height / 2.0;
        let cx = lerp(cx_off, cx_on, p);
        let cy = track.y + TRACK.height / 2.0;
        // State layer (40 dp halo) on hover / press.
        let halo = state.hover.value.max(state.press.value).clamp(0.0, 1.0);
        if halo > 0.01 {
            let r = 20.0;
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(Point::new(cx - r, cy - r), Size::new(2.0 * r, 2.0 * r)),
                    border: Border {
                        radius: r.into(),
                        ..Border::default()
                    },
                    shadow: Shadow::default(),
                    snap: false,
                },
                Background::Color(Color {
                    a: c.state_layer.a * halo,
                    ..c.state_layer
                }),
            );
        }
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(Point::new(cx - tw / 2.0, cy - th / 2.0), Size::new(tw, th)),
                border: Border {
                    radius: (th / 2.0).into(),
                    ..Border::default()
                },
                shadow: Shadow::default(),
                snap: false,
            },
            Background::Color(lerp_color(c.thumb_off, c.thumb_on, pc)),
        );
        // Check mark when on.
        if pc > 0.5 {
            let alpha = ((pc - 0.5) * 2.0).clamp(0.0, 1.0);
            renderer.fill_text(
                iced::advanced::Text {
                    content: crate::ui::icons::CHECK.to_string(),
                    bounds: Size::new(d, d),
                    size: iced::Pixels(16.0),
                    line_height: iced::advanced::text::LineHeight::Relative(1.0),
                    font: crate::ui::widgets::SYMBOLS,
                    align_x: iced::advanced::text::Alignment::Center,
                    align_y: iced::alignment::Vertical::Center,
                    shaping: iced::advanced::text::Shaping::Basic,
                    wrapping: iced::advanced::text::Wrapping::None,
                },
                Point::new(cx, cy),
                Color {
                    a: alpha,
                    ..c.icon_on
                },
                b,
            );
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a, Message: Clone + 'a> From<Switch<'a, Message>> for Element<'a, Message> {
    fn from(s: Switch<'a, Message>) -> Self {
        Element::new(s)
    }
}

// ---------------------------------------------------------------------------
// Slider (M3 Expressive)
// ---------------------------------------------------------------------------

/// Colours of the expressive slider.
#[derive(Clone, Copy, Debug)]
pub struct SliderColors {
    pub active: Color,
    pub inactive: Color,
    pub handle: Color,
    pub stop_on_active: Color,
    pub stop_on_inactive: Color,
    pub bubble: Color,
    pub bubble_text: Color,
}

impl SliderColors {
    pub fn standard(r: &crate::theme::Roles) -> Self {
        Self {
            active: r.primary,
            inactive: r.secondary_container,
            handle: r.primary,
            stop_on_active: r.on_primary,
            stop_on_inactive: r.primary,
            bubble: r.inverse_surface,
            bubble_text: r.inverse_on_surface,
        }
    }

    /// On a coloured container (the gaps around the handle are transparent,
    /// so only the inactive track needs to stand out from it).
    pub fn on_container(r: &crate::theme::Roles, container: Color) -> Self {
        let mut c = Self::standard(r);
        if container == r.primary_container {
            c.inactive = crate::theme::with_alpha(r.on_primary_container, 0.18);
        }
        c
    }
}

type FormatFn<'a> = Box<dyn Fn(f32) -> String + 'a>;

pub struct Slider<'a, Message> {
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    step: f32,
    stops: Vec<f32>,
    on_change: Box<dyn Fn(f32) -> Message + 'a>,
    on_release: Option<Message>,
    colors: Option<SliderColors>,
    height: f32,
    format: Option<FormatFn<'a>>,
}

/// Expressive slider (see the module documentation).
pub fn slider<'a, Message>(
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Slider<'a, Message> {
    Slider {
        range,
        value,
        step: 0.01,
        stops: Vec::new(),
        on_change: Box::new(on_change),
        on_release: None,
        colors: None,
        height: 44.0,
        format: None,
    }
}

impl<'a, Message> Slider<'a, Message> {
    pub fn step(mut self, step: f32) -> Self {
        self.step = step.max(f32::EPSILON);
        self
    }

    /// Stop indicators (also magnetic detents), e.g. `&[1.0]` for 100 %.
    pub fn stops(mut self, stops: &[f32]) -> Self {
        self.stops = stops.to_vec();
        self
    }

    pub fn colors(mut self, colors: SliderColors) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height.max(28.0);
        self
    }

    pub fn on_release(mut self, message: Message) -> Self {
        self.on_release = Some(message);
        self
    }

    /// Text of the value bubble shown while dragging (default: percent of
    /// the range for 0–1 / 0–2 ranges, the value otherwise).
    pub fn format(mut self, f: impl Fn(f32) -> String + 'a) -> Self {
        self.format = Some(Box::new(f));
        self
    }

    fn fraction(&self, v: f32) -> f32 {
        let (min, max) = (*self.range.start(), *self.range.end());
        if max <= min {
            0.0
        } else {
            ((v - min) / (max - min)).clamp(0.0, 1.0)
        }
    }

    fn value_at(&self, fraction: f32) -> f32 {
        let (min, max) = (*self.range.start(), *self.range.end());
        let raw = min + fraction.clamp(0.0, 1.0) * (max - min);
        let mut v = (min + ((raw - min) / self.step).round() * self.step).clamp(min, max);
        // Detents: a stop "catches" the handle within 2 % of the range.
        for &stop in &self.stops {
            if (raw - stop).abs() <= (max - min) * 0.02 {
                v = stop;
            }
        }
        v
    }

    fn label(&self, v: f32) -> String {
        match &self.format {
            Some(f) => f(v),
            None if *self.range.start() >= 0.0 && *self.range.end() <= 2.0 => {
                format!("{:.0} %", v * 100.0)
            }
            None if self.step >= 1.0 => format!("{v:.0}"),
            None => format!("{v:.2}"),
        }
    }
}

struct SliderState {
    dragging: bool,
    /// Visual value: follows the real value through a spring (elastic).
    shown: Anim,
    /// Track thickness factor: 0 rest, 1 hover, 2 dragging.
    thick: Anim,
    /// Handle squash / stretch from the drag speed.
    stretch: Anim,
    /// Rubber band past the ends, in pixels (signed).
    overdrag: Anim,
    bubble: Anim,
    hover: Anim,
    grab: Anim,
    last_x: Option<(f32, Instant)>,
    velocity: f32,
    last: Option<Instant>,
    initialised: bool,
}

/// Snappy, slightly elastic: the handle trails the finger by a few ms.
const FOLLOW_SPEC: SpringSpec = SpringSpec {
    damping: 0.78,
    stiffness: 1600.0,
};
/// Bouncy rubber band and handle wobble.
const RUBBER_SPEC: SpringSpec = SpringSpec {
    damping: 0.42,
    stiffness: 700.0,
};

impl<'a, Message: Clone + 'a> Slider<'a, Message> {
    fn geometry(&self, bounds: Rectangle) -> (f32, f32, f32) {
        // Track ends, inset so the handle never leaves the widget.
        let x0 = bounds.x + 4.0;
        let x1 = bounds.x + bounds.width - 4.0;
        (x0, x1.max(x0 + 1.0), bounds.y + bounds.height / 2.0)
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, Theme, Renderer> for Slider<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SliderState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SliderState {
            dragging: false,
            shown: Anim::new(self.value),
            thick: Anim::new(0.0),
            stretch: Anim::new(0.0),
            overdrag: Anim::new(0.0),
            bubble: Anim::new(0.0),
            hover: Anim::new(0.0),
            grab: Anim::new(0.0),
            last_x: None,
            velocity: 0.0,
            last: None,
            initialised: true,
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(
            Length::Fill,
            Length::Fixed(self.height),
            Size::new(160.0, self.height),
        ))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let (x0, x1, _) = self.geometry(bounds);
        let state = tree.state.downcast_mut::<SliderState>();
        if !state.initialised {
            state.shown = Anim::new(self.value);
            state.initialised = true;
        }
        let grab_area = bounds.expand(4.0);
        let position = |cursor: mouse::Cursor| {
            cursor
                .position()
                .or_else(|| cursor.land().position())
                .map(|p| p.x)
        };
        let mut moved_to: Option<f32> = None;
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerPressed { .. }) => {
                if cursor.is_over(grab_area) {
                    state.dragging = true;
                    state.velocity = 0.0;
                    state.last_x = None;
                    moved_to = position(cursor);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position: p })
            | Event::Touch(touch::Event::FingerMoved { position: p, .. }) => {
                if state.dragging {
                    moved_to = Some(p.x);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerLifted { .. })
            | Event::Touch(touch::Event::FingerLost { .. })
                if state.dragging =>
            {
                state.dragging = false;
                state.overdrag.target = 0.0;
                state.stretch.target = 0.0;
                state.last_x = None;
                if let Some(m) = &self.on_release {
                    shell.publish(m.clone());
                }
                shell.capture_event();
            }
            _ => {}
        }
        if let Some(x) = moved_to {
            let now = Instant::now();
            if let Some((px, pt)) = state.last_x {
                let dt = now.saturating_duration_since(pt).as_secs_f32().max(1e-3);
                let v = (x - px) / dt;
                state.velocity = state.velocity * 0.6 + v * 0.4;
            }
            state.last_x = Some((x, now));
            let f = (x - x0) / (x1 - x0);
            // Past the ends: resistance grows with the distance.
            let over = if x < x0 {
                -((x0 - x).min(240.0)).sqrt() * 1.6
            } else if x > x1 {
                ((x - x1).min(240.0)).sqrt() * 1.6
            } else {
                0.0
            };
            state.overdrag.target = over;
            state.overdrag.value = over; // follows the finger directly
            state.stretch.target = (state.velocity.abs() / 1800.0).clamp(0.0, 1.0);
            let v = self.value_at(f);
            if (v - self.value).abs() > f32::EPSILON {
                shell.publish((self.on_change)(v));
            }
        }
        let over = cursor.is_over(grab_area);
        state.hover.target = if over || state.dragging { 1.0 } else { 0.0 };
        state.grab.target = if state.dragging { 1.0 } else { 0.0 };
        state.thick.target = if state.dragging {
            2.0
        } else if over {
            1.0
        } else {
            0.0
        };
        state.bubble.target = if state.dragging { 1.0 } else { 0.0 };
        state.shown.target = self.value;
        if !state.dragging {
            state.stretch.target = 0.0;
        }
        let moving = |s: &SliderState| {
            s.shown.moving()
                || s.thick.moving()
                || s.stretch.moving()
                || s.overdrag.moving()
                || s.bubble.moving()
                || s.hover.moving()
                || s.grab.moving()
        };
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let dt = crate::theme::motion::scaled(
                state
                    .last
                    .map(|t| now.saturating_duration_since(t).as_secs_f32())
                    .unwrap_or(1.0 / 60.0)
                    .min(0.05),
            );
            // Velocity decays when the finger stops.
            if state.dragging
                && state
                    .last_x
                    .is_some_and(|(_, t)| t.elapsed().as_secs_f32() > 0.05)
            {
                state.velocity *= 0.5;
                state.stretch.target = (state.velocity.abs() / 1800.0).clamp(0.0, 1.0);
            }
            state.shown.step(dt, FOLLOW_SPEC);
            state.thick.step(dt, motion::EXPRESSIVE_FAST_SPATIAL);
            state.stretch.step(dt, RUBBER_SPEC);
            state.overdrag.step(dt, RUBBER_SPEC);
            state.bubble.step(dt, motion::EXPRESSIVE_FAST_SPATIAL);
            state.hover.step(dt, HOVER_SPEC);
            state.grab.step(dt, PRESS_SPEC);
            if moving(state) {
                state.last = Some(*now);
                shell.request_redraw();
            } else {
                state.last = None;
            }
        } else if moving(state) {
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        use iced::advanced::text::Renderer as _;
        let state = tree.state.downcast_ref::<SliderState>();
        let palette = theme.extended_palette();
        let c = self.colors.unwrap_or(SliderColors {
            active: palette.primary.base.color,
            inactive: palette.secondary.weak.color,
            handle: palette.primary.base.color,
            stop_on_active: palette.primary.base.text,
            stop_on_inactive: palette.primary.base.color,
            bubble: palette.background.strong.color,
            bubble_text: palette.background.strong.text,
        });
        let bounds = layout.bounds();
        let (x0, x1, cy) = self.geometry(bounds);
        let max_t = (bounds.height * 0.62).min(26.0);
        // 16 dp at rest, 20 on hover, 26 while dragging (springy).
        let t = lerp(16.0_f32.min(max_t * 0.7), max_t, state.thick.value / 2.0).max(4.0);
        let over = state.overdrag.value;
        // The pulled end stretches, the other one stays put.
        let (x0, x1) = if over < 0.0 {
            (x0 + over, x1)
        } else {
            (x0, x1 + over)
        };
        let f = self.fraction(state.shown.value);
        let hx = x0 + f * (x1 - x0) + over * 0.15;
        // Handle: 4 dp wide, 2.5 dp when grabbed (M3), squashed / stretched
        // by the drag speed: wider and shorter when fast.
        let grab = state.grab.value;
        let stretch = state.stretch.value.max(0.0);
        let hw = lerp(4.0, 2.5, grab.clamp(0.0, 1.0)) + 0.8 * state.hover.value.clamp(0.0, 1.0)
            - 0.8 * grab.clamp(0.0, 1.0)
            + stretch * 5.0;
        let hh = (t + 20.0 + 4.0 * grab - stretch * 8.0).min(bounds.height + 6.0);
        let gap = 6.0;
        let inner = lerp(2.0, 6.0, stretch.min(1.0)).min(t / 2.0);
        let outer = t / 2.0;
        let quad = |r: &mut Renderer, rect: Rectangle, radius: iced::border::Radius, col: Color| {
            if rect.width > 0.5 {
                r.fill_quad(
                    renderer::Quad {
                        bounds: rect,
                        border: Border {
                            radius,
                            ..Border::default()
                        },
                        shadow: Shadow::default(),
                        snap: false,
                    },
                    Background::Color(col),
                );
            }
        };
        // Active track (left of the handle).
        let a_end = hx - hw / 2.0 - gap;
        quad(
            renderer,
            Rectangle::new(
                Point::new(x0, cy - t / 2.0),
                Size::new((a_end - x0).max(0.0), t),
            ),
            iced::border::Radius {
                top_left: outer,
                bottom_left: outer,
                top_right: inner,
                bottom_right: inner,
            },
            c.active,
        );
        // Inactive track.
        let i_start = hx + hw / 2.0 + gap;
        quad(
            renderer,
            Rectangle::new(
                Point::new(i_start, cy - t / 2.0),
                Size::new((x1 - i_start).max(0.0), t),
            ),
            iced::border::Radius {
                top_left: inner,
                bottom_left: inner,
                top_right: outer,
                bottom_right: outer,
            },
            c.inactive,
        );
        // Stop indicators: the end of the track, then the detents.
        let dot = |r: &mut Renderer, x: f32, col: Color| {
            let d = 4.0;
            r.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(Point::new(x - d / 2.0, cy - d / 2.0), Size::new(d, d)),
                    border: Border {
                        radius: (d / 2.0).into(),
                        ..Border::default()
                    },
                    shadow: Shadow::default(),
                    snap: false,
                },
                Background::Color(col),
            );
        };
        if x1 - i_start > outer + 6.0 {
            dot(renderer, x1 - outer, c.stop_on_inactive);
        }
        for &stop in &self.stops {
            let sx = x0 + self.fraction(stop) * (x1 - x0);
            if (sx - hx).abs() > hw / 2.0 + gap + 3.0 {
                let col = if sx < hx {
                    c.stop_on_active
                } else {
                    c.stop_on_inactive
                };
                dot(renderer, sx, col);
            }
        }
        // Handle.
        quad(
            renderer,
            Rectangle::new(Point::new(hx - hw / 2.0, cy - hh / 2.0), Size::new(hw, hh)),
            (hw / 2.0).into(),
            c.handle,
        );
        // Value bubble.
        let b = state.bubble.value;
        if b > 0.02 {
            let scale = b.clamp(0.0, 1.2);
            let (bw, bh) = (52.0 * scale, 32.0 * scale);
            let by = cy - hh / 2.0 - 6.0 - bh;
            quad(
                renderer,
                Rectangle::new(Point::new(hx - bw / 2.0, by), Size::new(bw, bh)),
                (bh / 2.0).into(),
                Color {
                    a: c.bubble.a * b.clamp(0.0, 1.0),
                    ..c.bubble
                },
            );
            if scale > 0.5 {
                renderer.fill_text(
                    iced::advanced::Text {
                        content: self.label(self.value),
                        bounds: Size::new(bw, bh),
                        size: iced::Pixels(13.0 * scale.min(1.0)),
                        line_height: iced::advanced::text::LineHeight::Relative(1.0),
                        font: crate::ui::widgets::weight(iced::font::Weight::Semibold),
                        align_x: iced::advanced::text::Alignment::Center,
                        align_y: iced::alignment::Vertical::Center,
                        shaping: iced::advanced::text::Shaping::Basic,
                        wrapping: iced::advanced::text::Wrapping::None,
                    },
                    Point::new(hx, by + bh / 2.0),
                    Color {
                        a: b.clamp(0.0, 1.0),
                        ..c.bubble_text
                    },
                    Rectangle::new(Point::new(hx - bw, by - 4.0), Size::new(bw * 2.0, bh + 8.0)),
                );
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<SliderState>();
        if state.dragging {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(layout.bounds().expand(4.0)) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a, Message: Clone + 'a> From<Slider<'a, Message>> for Element<'a, Message> {
    fn from(s: Slider<'a, Message>) -> Self {
        Element::new(s)
    }
}

// ---------------------------------------------------------------------------
// Expressive shape badge (M3 Expressive "cookie" shapes)
// ---------------------------------------------------------------------------

/// Scalloped circle with `lobes` bumps (M3 Expressive Cookie 9 / Sunny…).
pub struct ShapeBadge {
    pub lobes: u32,
    pub depth: f32,
    pub color: Color,
}

impl<Message> canvas::Program<Message> for ShapeBadge {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let c = frame.center();
        let r = bounds.width.min(bounds.height) / 2.0;
        let base = r * (1.0 - self.depth);
        let amp = r * self.depth;
        let steps = (self.lobes * 16).max(48);
        let path = canvas::Path::new(|p| {
            for i in 0..=steps {
                let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                // Soft scallops like the M3 Expressive cookie shapes.
                let w = 0.5 + 0.5 * (a * self.lobes as f32).cos();
                let rr = base + amp * w;
                let pt = Point::new(c.x + rr * a.sin(), c.y - rr * a.cos());
                if i == 0 {
                    p.move_to(pt);
                } else {
                    p.line_to(pt);
                }
            }
            p.close();
        });
        frame.fill(&path, self.color);
        vec![frame.into_geometry()]
    }
}

/// An icon centred on an expressive scalloped shape.
pub fn shape_badge<'a, Message: 'a>(
    glyph: char,
    size: f32,
    lobes: u32,
    shape: Color,
    icon: Color,
) -> Element<'a, Message> {
    iced::widget::stack![
        canvas(ShapeBadge {
            lobes,
            depth: 0.075,
            color: shape,
        })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size)),
        iced::widget::container(crate::ui::widgets::icon_filled(glyph, size * 0.46).color(icon))
            .center_x(Length::Fixed(size))
            .center_y(Length::Fixed(size)),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_settles_and_overshoots_on_press() {
        let mut a = Anim::new(0.0);
        a.target = 1.0;
        let mut peak: f32 = 0.0;
        for _ in 0..120 {
            a.step(1.0 / 60.0, PRESS_SPEC);
            peak = peak.max(a.value);
        }
        assert!(!a.moving());
        assert_eq!(a.value, 1.0);
        assert!(
            peak > 1.0,
            "expressive press should overshoot slightly ({peak})"
        );
        let mut h = Anim::new(0.0);
        h.target = 1.0;
        let mut hpeak: f32 = 0.0;
        for _ in 0..120 {
            h.step(1.0 / 60.0, HOVER_SPEC);
            hpeak = hpeak.max(h.value);
        }
        assert!(hpeak <= 1.0 + 1e-4, "hover colour must not overshoot");
    }

    #[test]
    fn slider_snaps_to_steps_and_detents() {
        let s = slider(0.0..=2.0, 1.0, |v| v).step(0.01).stops(&[1.0]);
        assert_eq!(s.value_at(0.0), 0.0);
        assert_eq!(s.value_at(1.0), 2.0);
        // 1.03 is within 2 % of the range (0.04) from the 100 % stop.
        assert_eq!(s.value_at(1.03 / 2.0), 1.0);
        assert!((s.value_at(0.75) - 1.5).abs() < 1e-5);
        assert_eq!(s.label(1.2), "120 %");
        let n = slider(4.0..=60.0, 14.0, |v| v).step(1.0);
        assert_eq!(n.value_at(0.5), 32.0);
        assert_eq!(n.label(32.0), "32");
    }

    #[test]
    fn blend_interpolates_radius_and_colour() {
        let a = Style {
            background: Some(Background::Color(Color::BLACK)),
            border: Border {
                radius: 20.0.into(),
                ..Border::default()
            },
            ..Style::default()
        };
        let b = Style {
            background: Some(Background::Color(Color::WHITE)),
            border: Border {
                radius: 12.0.into(),
                ..Border::default()
            },
            ..Style::default()
        };
        let m = blend(&a, &b, 0.5);
        assert_eq!(m.border.radius.top_left, 16.0);
        assert_eq!(
            m.background,
            Some(Background::Color(lerp_color(
                Color::BLACK,
                Color::WHITE,
                0.5
            )))
        );
    }
}
