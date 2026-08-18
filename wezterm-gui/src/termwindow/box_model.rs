#![allow(dead_code)]
use crate::color::LinearRgba;
use crate::customglyph::{BlockKey, Poly};
use crate::glyphcache::CachedGlyph;
use crate::quad::{QuadImpl, QuadTrait, TripleLayerQuadAllocator, TripleLayerQuadAllocatorTrait};
use crate::termwindow::{
    ColorEase, MouseCapture, RenderState, TermWindowNotif, UIItem, UIItemType,
};
use crate::utilsprites::RenderMetrics;
use ::window::bitmaps::TextureRect;
use ::window::{PointF, RectF, SizeF, WindowOps};
use anyhow::anyhow;
use config::{Dimension, DimensionContext};
use finl_unicode::grapheme_clusters::Graphemes;
use std::cell::RefCell;
use std::rc::Rc;
use termwiz::cell::{grapheme_column_width, Presentation};
use termwiz::surface::Line;
use wezterm_font::units::PixelUnit;
use wezterm_font::LoadedFont;
use wezterm_term::color::{ColorAttribute, ColorPalette};
use window::bitmaps::atlas::Sprite;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Top,
    Bottom,
    Middle,
}

impl Default for VerticalAlign {
    fn default() -> VerticalAlign {
        VerticalAlign::Top
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayType {
    Block,
    Inline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Float {
    None,
    Right,
}

impl Default for Float {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PixelDimension {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PixelSizedPoly {
    pub poly: &'static [Poly],
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SizedPoly {
    pub poly: &'static [Poly],
    pub width: Dimension,
    pub height: Dimension,
}

impl SizedPoly {
    pub fn to_pixels(&self, context: &LayoutContext) -> PixelSizedPoly {
        PixelSizedPoly {
            poly: self.poly,
            width: self.width.evaluate_as_pixels(context.width),
            height: self.height.evaluate_as_pixels(context.height),
        }
    }

    pub fn none() -> Self {
        Self {
            poly: &[],
            width: Dimension::default(),
            height: Dimension::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PixelCorners {
    pub top_left: PixelSizedPoly,
    pub top_right: PixelSizedPoly,
    pub bottom_left: PixelSizedPoly,
    pub bottom_right: PixelSizedPoly,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Corners {
    pub top_left: SizedPoly,
    pub top_right: SizedPoly,
    pub bottom_left: SizedPoly,
    pub bottom_right: SizedPoly,
}

impl Corners {
    pub fn to_pixels(&self, context: &LayoutContext) -> PixelCorners {
        PixelCorners {
            top_left: self.top_left.to_pixels(context),
            top_right: self.top_right.to_pixels(context),
            bottom_left: self.bottom_left.to_pixels(context),
            bottom_right: self.bottom_right.to_pixels(context),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoxDimension {
    pub left: Dimension,
    pub top: Dimension,
    pub right: Dimension,
    pub bottom: Dimension,
}

impl BoxDimension {
    pub const fn new(dim: Dimension) -> Self {
        Self {
            left: dim,
            top: dim,
            right: dim,
            bottom: dim,
        }
    }

    pub fn to_pixels(&self, context: &LayoutContext) -> PixelDimension {
        PixelDimension {
            left: self.left.evaluate_as_pixels(context.width),
            top: self.top.evaluate_as_pixels(context.height),
            right: self.right.evaluate_as_pixels(context.width),
            bottom: self.bottom.evaluate_as_pixels(context.height),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InheritableColor {
    Inherited,
    Color(LinearRgba),
    Animated {
        color: LinearRgba,
        alt_color: LinearRgba,
        ease: Rc<RefCell<ColorEase>>,
        one_shot: bool,
    },
}

impl Default for InheritableColor {
    fn default() -> Self {
        Self::Inherited
    }
}

impl From<LinearRgba> for InheritableColor {
    fn from(color: LinearRgba) -> InheritableColor {
        InheritableColor::Color(color)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BorderColor {
    pub left: LinearRgba,
    pub top: LinearRgba,
    pub right: LinearRgba,
    pub bottom: LinearRgba,
}

impl BorderColor {
    pub const fn new(color: LinearRgba) -> Self {
        Self {
            left: color,
            top: color,
            right: color,
            bottom: color,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ElementColors {
    pub border: BorderColor,
    pub bg: InheritableColor,
    pub text: InheritableColor,
}

struct ResolvedColor {
    color: LinearRgba,
    alt_color: LinearRgba,
    mix_value: f32,
}

impl ResolvedColor {
    fn apply(&self, quad: &mut QuadImpl) {
        quad.set_fg_color(self.color);
        quad.set_alt_color_and_mix_value(self.alt_color, self.mix_value);
    }
}

impl From<LinearRgba> for ResolvedColor {
    fn from(color: LinearRgba) -> Self {
        Self {
            color,
            alt_color: color,
            mix_value: 0.,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Element {
    pub item_type: Option<UIItemType>,
    pub vertical_align: VerticalAlign,
    pub zindex: i8,
    pub display: DisplayType,
    pub float: Float,
    pub padding: BoxDimension,
    pub margin: BoxDimension,
    pub border: BoxDimension,
    pub border_corners: Option<Corners>,
    pub colors: ElementColors,
    pub hover_colors: Option<ElementColors>,
    pub font: Rc<LoadedFont>,
    pub content: ElementContent,
    pub presentation: Option<Presentation>,
    pub line_height: Option<f64>,
    pub max_width: Option<Dimension>,
    pub min_width: Option<Dimension>,
    pub min_height: Option<Dimension>,
    /// Centre text content in whatever space `min_width` adds.
    ///
    /// Off by default, because every other caller lays text out at the content
    /// origin and expects to keep doing so. `min_width` on its own only widens
    /// the box: the glyphs still start at its left edge, so a label in a box
    /// sized for a touch target ends up in its top-left corner rather than the
    /// middle of the key the user is aiming at.
    pub center_x: bool,
    /// Centre text content in whatever space `min_height` adds.
    ///
    /// Note that this is *not* `VerticalAlign::Middle`, which moves this element
    /// within its parent rather than moving the text within this element. A row
    /// that gives itself a height and then asks for `Middle` to centre its own
    /// label gets neither: the label stays at the top of the row, and the row
    /// itself is pushed to the middle of whatever contains it.
    pub center_y: bool,
}

impl Element {
    pub fn new(font: &Rc<LoadedFont>, content: ElementContent) -> Self {
        Self {
            item_type: None,
            zindex: 0,
            display: DisplayType::Inline,
            float: Float::None,
            padding: BoxDimension::default(),
            margin: BoxDimension::default(),
            border: BoxDimension::default(),
            border_corners: None,
            vertical_align: VerticalAlign::default(),
            colors: ElementColors::default(),
            hover_colors: None,
            font: Rc::clone(font),
            content,
            presentation: None,
            line_height: None,
            max_width: None,
            min_width: None,
            min_height: None,
            center_x: false,
            center_y: false,
        }
    }

    pub fn with_line(font: &Rc<LoadedFont>, line: &Line, palette: &ColorPalette) -> Self {
        let mut content: Vec<Element> = vec![];
        let mut prior_attr = None;

        for cluster in line.cluster(None) {
            // Clustering may introduce cluster boundaries when the text hasn't actually
            // changed style. Undo that here.
            // There's still an issue where the style does actually change and we
            // subsequently don't clip the element.
            // <https://github.com/wezterm/wezterm/issues/2560>
            if let Some(prior) = content.last_mut() {
                let (fg, bg) = prior_attr.as_ref().unwrap();
                if cluster.attrs.background() == *bg && cluster.attrs.foreground() == *fg {
                    if let ElementContent::Text(t) = &mut prior.content {
                        t.push_str(&cluster.text);
                        continue;
                    }
                }
            }

            let child =
                Element::new(font, ElementContent::Text(cluster.text)).colors(ElementColors {
                    border: BorderColor::default(),
                    bg: if cluster.attrs.background() == ColorAttribute::Default {
                        InheritableColor::Inherited
                    } else {
                        palette
                            .resolve_bg(cluster.attrs.background())
                            .to_linear()
                            .into()
                    },
                    text: if cluster.attrs.foreground() == ColorAttribute::Default {
                        InheritableColor::Inherited
                    } else {
                        palette
                            .resolve_fg(cluster.attrs.foreground())
                            .to_linear()
                            .into()
                    },
                });

            content.push(child);
            prior_attr.replace((cluster.attrs.foreground(), cluster.attrs.background()));
        }

        Self::new(font, ElementContent::Children(content))
    }

    pub fn vertical_align(mut self, align: VerticalAlign) -> Self {
        self.vertical_align = align;
        self
    }

    pub fn item_type(mut self, item_type: UIItemType) -> Self {
        self.item_type.replace(item_type);
        self
    }

    pub fn display(mut self, display: DisplayType) -> Self {
        self.display = display;
        self
    }

    pub fn float(mut self, float: Float) -> Self {
        self.float = float;
        self
    }

    pub fn colors(mut self, colors: ElementColors) -> Self {
        self.colors = colors;
        self
    }

    pub fn hover_colors(mut self, colors: Option<ElementColors>) -> Self {
        self.hover_colors = colors;
        self
    }

    pub fn line_height(mut self, line_height: Option<f64>) -> Self {
        self.line_height = line_height;
        self
    }

    pub fn zindex(mut self, zindex: i8) -> Self {
        self.zindex = zindex;
        self
    }

    pub fn padding(mut self, padding: BoxDimension) -> Self {
        self.padding = padding;
        self
    }

    pub fn border(mut self, border: BoxDimension) -> Self {
        self.border = border;
        self
    }

    pub fn border_corners(mut self, corners: Option<Corners>) -> Self {
        self.border_corners = corners;
        self
    }

    pub fn margin(mut self, margin: BoxDimension) -> Self {
        self.margin = margin;
        self
    }

    pub fn max_width(mut self, width: Option<Dimension>) -> Self {
        self.max_width = width;
        self
    }

    pub fn min_width(mut self, width: Option<Dimension>) -> Self {
        self.min_width = width;
        self
    }

    pub fn min_height(mut self, height: Option<Dimension>) -> Self {
        self.min_height = height;
        self
    }

    /// Centre the text on both axes within `min_width`/`min_height`.
    pub fn center_content(mut self, center: bool) -> Self {
        self.center_x = center;
        self.center_y = center;
        self
    }

    /// Centre the text vertically within `min_height`, leaving it left-aligned.
    pub fn center_content_vertically(mut self, center: bool) -> Self {
        self.center_y = center;
        self
    }
}

#[derive(Debug, Clone)]
pub enum ElementContent {
    Text(String),
    Children(Vec<Element>),
    Poly { line_width: isize, poly: SizedPoly },
}

pub struct LayoutContext<'a> {
    pub width: DimensionContext,
    pub height: DimensionContext,
    pub bounds: RectF,
    pub metrics: &'a RenderMetrics,
    pub gl_state: &'a RenderState,
    pub zindex: i8,
}

#[derive(Debug, Clone)]
pub struct ComputedElement {
    pub item_type: Option<UIItemType>,
    pub zindex: i8,
    /// The outer bounds of the element box (its margin)
    pub bounds: RectF,
    /// The outer bounds of the area enclosed by its border
    pub border_rect: RectF,
    pub border: PixelDimension,
    pub border_corners: Option<PixelCorners>,
    pub colors: ElementColors,
    pub hover_colors: Option<ElementColors>,
    /// The outer bounds of the area enclosed by the padding
    pub padding: RectF,
    /// The outer bounds of the content
    pub content_rect: RectF,
    pub baseline: f32,

    /// Restricts this element and its children to a rectangle, in the same
    /// window coordinates as `bounds`. Set after layout by a caller that pans
    /// content within a fixed viewport; nothing in `compute_element` produces
    /// one.
    ///
    /// The clip bounds both the rendering and the `UIItem`s. Clipping the hit
    /// rectangles is not decoration: a scrolled key that has slid under a
    /// pinned one would otherwise still answer taps in the pinned key's
    /// rectangle, which is a `CTRL` that intermittently types `ESC`.
    pub clip: Option<RectF>,

    pub content: ComputedElementContent,
}

/// The overlap of two clips, either of which may be absent.
fn intersect_clip(outer: Option<RectF>, inner: Option<RectF>) -> Option<RectF> {
    match (outer, inner) {
        (Some(outer), Some(inner)) => Some(outer.intersection(&inner).unwrap_or_else(RectF::zero)),
        (Some(rect), None) | (None, Some(rect)) => Some(rect),
        (None, None) => None,
    }
}

/// Trim a textured quad to `clip`, in the destination's own coordinate space.
///
/// Both the destination rectangle and the texture rectangle are cut by the same
/// fractions, so the surviving part of the sprite lands exactly where it would
/// have without the clip. Returns `None` when nothing of it survives.
fn clip_textured_quad(
    dest: RectF,
    tex: TextureRect,
    clip: Option<RectF>,
) -> Option<(RectF, TextureRect)> {
    let clip = match clip {
        Some(clip) => clip,
        None => return Some((dest, tex)),
    };

    if dest.width() <= 0. || dest.height() <= 0. {
        return None;
    }

    let trimmed = clip.intersection(&dest)?;
    if trimmed.width() <= 0. || trimmed.height() <= 0. {
        return None;
    }
    if trimmed == dest {
        return Some((dest, tex));
    }

    let fx0 = (trimmed.min_x() - dest.min_x()) / dest.width();
    let fx1 = (trimmed.max_x() - dest.min_x()) / dest.width();
    let fy0 = (trimmed.min_y() - dest.min_y()) / dest.height();
    let fy1 = (trimmed.max_y() - dest.min_y()) / dest.height();

    let tex = euclid::rect(
        tex.min_x() + tex.width() * fx0,
        tex.min_y() + tex.height() * fy0,
        tex.width() * (fx1 - fx0),
        tex.height() * (fy1 - fy0),
    );

    Some((trimmed, tex))
}

impl ComputedElement {
    pub fn translate(&mut self, delta: euclid::Vector2D<f32, PixelUnit>) {
        self.bounds = self.bounds.translate(delta);
        self.border_rect = self.border_rect.translate(delta);
        self.padding = self.padding.translate(delta);
        self.content_rect = self.content_rect.translate(delta);
        // The clip is in the same coordinates as the rest, so it moves too.
        // A caller that wants a clip fixed in the window sets it after any
        // translation.
        self.clip = self.clip.map(|clip| clip.translate(delta));

        match &mut self.content {
            ComputedElementContent::Children(kids) => {
                for kid in kids {
                    kid.translate(delta)
                }
            }
            ComputedElementContent::Text(_) => {}
            ComputedElementContent::Poly { .. } => {}
        }
    }

    /// Restrict this element and everything under it to `clip`, in window
    /// coordinates.
    pub fn set_clip(&mut self, clip: RectF) {
        self.clip = Some(match self.clip {
            Some(existing) => existing.intersection(&clip).unwrap_or_else(RectF::zero),
            None => clip,
        });
    }

    pub fn ui_items(&self) -> Vec<UIItem> {
        let mut items = vec![];
        self.ui_item_impl(&mut items, None);
        items
    }

    fn ui_item_impl(&self, items: &mut Vec<UIItem>, clip: Option<RectF>) {
        let clip = intersect_clip(clip, self.clip);

        if let Some(item_type) = &self.item_type {
            let bounds = match clip {
                Some(clip) => clip.intersection(&self.bounds).unwrap_or_else(RectF::zero),
                None => self.bounds,
            };
            // A hit rectangle clipped away to nothing is not published at all,
            // rather than published as a zero-sized one: UIItem::hit_test uses
            // inclusive bounds, so an empty rectangle still matches its own
            // corner.
            if bounds.width() > 0. && bounds.height() > 0. {
                items.push(UIItem {
                    x: bounds.min_x().max(0.) as usize,
                    y: bounds.min_y().max(0.) as usize,
                    width: bounds.width().max(0.) as usize,
                    height: bounds.height().max(0.) as usize,
                    item_type: item_type.clone(),
                });
            }
        }

        match &self.content {
            ComputedElementContent::Text(_) => {}
            ComputedElementContent::Children(kids) => {
                for kid in kids {
                    kid.ui_item_impl(items, clip);
                }
            }
            ComputedElementContent::Poly { .. } => {}
        }
    }
}

#[derive(Debug, Clone)]
pub enum ComputedElementContent {
    Text(Vec<ElementCell>),
    Children(Vec<ComputedElement>),
    Poly {
        line_width: isize,
        poly: PixelSizedPoly,
    },
}

#[derive(Debug, Clone)]
pub enum ElementCell {
    Sprite(Sprite),
    Glyph(Rc<CachedGlyph>),
}

#[derive(Debug)]
struct Rects {
    padding: RectF,
    border_rect: RectF,
    bounds: RectF,
    content_rect: RectF,
    translate: euclid::Vector2D<f32, PixelUnit>,
}

/// Space added outside the content box, beyond the element's own padding.
///
/// This is how centring is expressed: the content box keeps its natural size
/// and the slack is pushed outwards, so the glyphs move rather than the box
/// growing around a label pinned to its corner.
#[derive(Debug, Default, Clone, Copy)]
struct ExtraPadding {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

/// Split the slack between a natural size and a minimum into leading and
/// trailing halves.
///
/// The odd pixel goes to the trailing side. Which side it lands on matters
/// less than that it lands somewhere: dropping it would leave the content box
/// a pixel narrower than the minimum asked for, so a row of keys would drift
/// left of the hit rectangles that were laid out from the same minimum.
fn center_slack(natural: f32, minimum: f32) -> (f32, f32) {
    let slack = (minimum - natural).max(0.);
    let leading = (slack / 2.).floor();
    (leading, slack - leading)
}

impl Element {
    fn compute_rects(&self, context: &LayoutContext, content_rect: RectF) -> Rects {
        self.compute_rects_with_extra(context, content_rect, ExtraPadding::default())
    }

    fn compute_rects_with_extra(
        &self,
        context: &LayoutContext,
        content_rect: RectF,
        extra: ExtraPadding,
    ) -> Rects {
        let mut padding = self.padding.to_pixels(context);
        padding.left += extra.left;
        padding.right += extra.right;
        padding.top += extra.top;
        padding.bottom += extra.bottom;
        let margin = self.margin.to_pixels(context);
        let border = self.border.to_pixels(context);

        let padding = euclid::rect(
            content_rect.min_x() - padding.left,
            content_rect.min_y() - padding.top,
            content_rect.width() + padding.left + padding.right,
            content_rect.height() + padding.top + padding.bottom,
        );

        let border_rect = euclid::rect(
            padding.min_x() - border.left,
            padding.min_y() - border.top,
            padding.width() + border.left + border.right,
            padding.height() + border.top + border.bottom,
        );

        let bounds = euclid::rect(
            border_rect.min_x() - margin.left,
            border_rect.min_y() - margin.top,
            border_rect.width() + margin.left + margin.right,
            border_rect.height() + margin.top + margin.bottom,
        );
        let translate = euclid::vec2(
            context.bounds.min_x() - bounds.min_x(),
            context.bounds.min_y() - bounds.min_y(),
        );
        Rects {
            padding: padding.translate(translate),
            border_rect: border_rect.translate(translate),
            bounds: bounds.translate(translate),
            content_rect: content_rect.translate(translate),
            translate,
        }
    }
}

impl super::TermWindow {
    pub fn compute_element<'a>(
        &self,
        context: &LayoutContext,
        element: &Element,
    ) -> anyhow::Result<ComputedElement> {
        let local_metrics;
        let local_context;
        let context = if let Some(line_height) = element.line_height {
            local_metrics = context.metrics.scale_line_height(line_height);
            local_context = LayoutContext {
                height: DimensionContext {
                    dpi: context.height.dpi,
                    pixel_max: context.height.pixel_max,
                    pixel_cell: context.height.pixel_cell * line_height as f32,
                },
                width: context.width,
                bounds: context.bounds,
                gl_state: context.gl_state,
                metrics: &local_metrics,
                zindex: context.zindex,
            };
            &local_context
        } else {
            context
        };
        let border_corners = element
            .border_corners
            .as_ref()
            .map(|c| c.to_pixels(context));
        let style = element.font.style();
        let border = element.border.to_pixels(context);
        let padding = element.padding.to_pixels(context);
        let baseline = context.height.pixel_cell + context.metrics.descender.get() as f32;
        let min_width = match element.min_width {
            Some(w) => w.evaluate_as_pixels(context.width),
            None => 0.0,
        };
        let min_height = match element.min_height {
            Some(h) => h.evaluate_as_pixels(context.height),
            None => 0.0,
        };

        let border_and_padding_width = border.left + border.right + padding.left + padding.right;

        let max_width = match element.max_width {
            Some(w) => {
                w.evaluate_as_pixels(context.width)
                    .min(context.bounds.width())
                    - border_and_padding_width
            }
            None => context.bounds.width() - border_and_padding_width,
        }
        .min((context.width.pixel_max - context.bounds.min_x()) - border_and_padding_width);

        match &element.content {
            ElementContent::Text(s) => {
                let window = self.window.as_ref().unwrap().clone();
                let direction = wezterm_bidi::Direction::LeftToRight;
                let infos = element.font.shape(
                    &s,
                    move || window.notify(TermWindowNotif::InvalidateShapeCache),
                    BlockKey::filter_out_synthetic,
                    element.presentation,
                    direction,
                    None,
                    None,
                )?;
                let mut computed_cells = vec![];
                let mut glyph_cache = context.gl_state.glyph_cache.borrow_mut();
                let mut pixel_width = 0.0;
                let mut x_pos = context.bounds.min_x();
                let mut min_y = 0.0f32;
                let max_x = context.bounds.min_x() + max_width;

                for info in infos {
                    let cell_start = &s[info.cluster as usize..];
                    let mut iter = Graphemes::new(cell_start).peekable();
                    let grapheme = iter
                        .next()
                        .ok_or_else(|| anyhow!("info.cluster didn't map into string"))?;
                    if let Some(key) = BlockKey::from_str(grapheme) {
                        if pixel_width + context.width.pixel_cell >= max_x {
                            break;
                        }
                        pixel_width += context.width.pixel_cell;
                        x_pos += context.width.pixel_cell;
                        let sprite = glyph_cache.cached_block(key, context.metrics)?;
                        computed_cells.push(ElementCell::Sprite(sprite));
                    } else {
                        let next_grapheme: Option<&str> = iter.peek().map(|s| *s);
                        let followed_by_space = next_grapheme == Some(" ");
                        let num_cells = grapheme_column_width(grapheme, None);
                        let glyph = glyph_cache.cached_glyph(
                            &info,
                            style,
                            followed_by_space,
                            &element.font,
                            context.metrics,
                            num_cells as u8,
                        )?;

                        if let Some(texture) = glyph.texture.as_ref() {
                            let x_pos = x_pos + (glyph.x_offset + glyph.bearing_x).get() as f32;
                            let width = texture.coords.size.width as f32 * glyph.scale as f32;
                            if x_pos + width >= max_x {
                                break;
                            }
                        } else if x_pos + glyph.x_advance.get() as f32 >= max_x {
                            break;
                        }

                        min_y =
                            min_y.min(baseline - (glyph.y_offset + glyph.bearing_y).get() as f32);

                        pixel_width += glyph.x_advance.get() as f32;
                        x_pos += glyph.x_advance.get() as f32;

                        computed_cells.push(ElementCell::Glyph(glyph));
                    }
                }

                // Without centring the box simply grows to the minimum and the
                // text stays at its origin. With it, the box stays the size of
                // the text and the slack becomes padding, which is what moves
                // the glyphs; the odd pixel goes to the right/bottom rather
                // than being lost to rounding.
                let (content_width, left, right) = if element.center_x {
                    let (left, right) = center_slack(pixel_width, min_width);
                    (pixel_width, left, right)
                } else {
                    (pixel_width.max(min_width), 0., 0.)
                };
                let (content_height, top, bottom) = if element.center_y {
                    let (top, bottom) = center_slack(context.height.pixel_cell, min_height);
                    (context.height.pixel_cell, top, bottom)
                } else {
                    (context.height.pixel_cell.max(min_height), 0., 0.)
                };

                let content_rect = euclid::rect(0., 0., content_width, content_height);
                let extra = ExtraPadding {
                    left,
                    right,
                    top,
                    bottom,
                };

                let rects = element.compute_rects_with_extra(context, content_rect, extra);

                Ok(ComputedElement {
                    item_type: element.item_type.clone(),
                    zindex: element.zindex + context.zindex,
                    baseline,
                    border,
                    border_corners,
                    colors: element.colors.clone(),
                    hover_colors: element.hover_colors.clone(),
                    bounds: rects.bounds,
                    border_rect: rects.border_rect,
                    padding: rects.padding,
                    content_rect: rects.content_rect,
                    clip: None,
                    content: ComputedElementContent::Text(computed_cells),
                })
            }
            ElementContent::Children(kids) => {
                let mut block_pixel_width: f32 = 0.;
                let mut block_pixel_height: f32 = 0.;
                let mut computed_kids = vec![];
                let mut max_x: f32 = 0.;
                let mut float_width: f32 = 0.;
                let mut y_coord: f32 = 0.;

                for child in kids {
                    if child.display == DisplayType::Block {
                        y_coord += block_pixel_height;
                        block_pixel_height = 0.;
                        block_pixel_width = 0.;
                    }

                    let bounds = match child.float {
                        Float::None => euclid::rect(
                            block_pixel_width,
                            y_coord,
                            context.bounds.max_x() - (context.bounds.min_x() + block_pixel_width),
                            context.bounds.max_y() - (context.bounds.min_y() + y_coord),
                        ),
                        Float::Right => euclid::rect(
                            0.,
                            y_coord,
                            context.bounds.width(),
                            context.bounds.max_y() - (context.bounds.min_y() + y_coord),
                        ),
                    };
                    let kid = self.compute_element(
                        &LayoutContext {
                            bounds,
                            gl_state: context.gl_state,
                            height: context.height,
                            metrics: context.metrics,
                            width: DimensionContext {
                                dpi: context.width.dpi,
                                pixel_cell: context.width.pixel_cell,
                                pixel_max: max_width,
                            },
                            zindex: context.zindex + element.zindex,
                        },
                        child,
                    )?;
                    match child.float {
                        Float::Right => {
                            float_width += float_width.max(kid.bounds.width());
                        }
                        Float::None => {
                            block_pixel_width += kid.bounds.width();
                            max_x = max_x.max(block_pixel_width);
                        }
                    }
                    block_pixel_height = block_pixel_height.max(kid.bounds.height());

                    computed_kids.push(kid);
                }

                // Respect min-width
                max_x = max_x.max(min_width);

                let mut float_max_x = (max_x + float_width).min(max_width);

                let pixel_height = (y_coord + block_pixel_height).max(min_height);

                for (kid, child) in computed_kids.iter_mut().zip(kids.iter()) {
                    match child.float {
                        Float::Right => {
                            max_x = max_x.max(float_max_x);
                            let x = float_max_x - kid.bounds.width();
                            float_max_x -= kid.bounds.width();
                            kid.translate(euclid::vec2(x, 0.));
                        }
                        _ => {}
                    }
                    match child.vertical_align {
                        VerticalAlign::Bottom => {
                            kid.translate(euclid::vec2(0., pixel_height - kid.bounds.height()));
                        }
                        VerticalAlign::Middle => {
                            kid.translate(euclid::vec2(
                                0.,
                                (pixel_height - kid.bounds.height()) / 2.0,
                            ));
                        }
                        VerticalAlign::Top => {}
                    }
                }

                computed_kids.sort_by(|a, b| a.zindex.cmp(&b.zindex));

                let content_rect = euclid::rect(0., 0., max_x.min(max_width), pixel_height);
                let rects = element.compute_rects(context, content_rect);

                for kid in &mut computed_kids {
                    kid.translate(rects.translate);
                }

                Ok(ComputedElement {
                    item_type: element.item_type.clone(),
                    zindex: element.zindex + context.zindex,
                    baseline,
                    border,
                    border_corners,
                    colors: element.colors.clone(),
                    hover_colors: element.hover_colors.clone(),
                    bounds: rects.bounds,
                    border_rect: rects.border_rect,
                    padding: rects.padding,
                    content_rect: rects.content_rect,
                    clip: None,
                    content: ComputedElementContent::Children(computed_kids),
                })
            }
            ElementContent::Poly { poly, line_width } => {
                let poly = poly.to_pixels(context);
                let content_rect = euclid::rect(0., 0., poly.width, poly.height.max(min_height));
                let rects = element.compute_rects(context, content_rect);

                Ok(ComputedElement {
                    item_type: element.item_type.clone(),
                    zindex: element.zindex + context.zindex,
                    baseline,
                    border,
                    border_corners,
                    colors: element.colors.clone(),
                    hover_colors: element.hover_colors.clone(),
                    bounds: rects.bounds,
                    border_rect: rects.border_rect,
                    padding: rects.padding,
                    content_rect: rects.content_rect,
                    clip: None,
                    content: ComputedElementContent::Poly {
                        poly,
                        line_width: *line_width,
                    },
                })
            }
        }
    }

    pub fn render_element<'a>(
        &self,
        element: &ComputedElement,
        gl_state: &RenderState,
        inherited_colors: Option<&ElementColors>,
    ) -> anyhow::Result<()> {
        self.render_element_clipped(element, gl_state, inherited_colors, None)
    }

    /// Render an element, restricted to `clip` in window coordinates.
    ///
    /// The quad allocators are batched, so there is no draw call to hang a
    /// scissor rectangle on and the clip has to be applied to the geometry:
    /// filled rectangles are trimmed, glyphs are trimmed in both position and
    /// texture coordinates, and a rounded corner that straddles the edge is
    /// dropped. Dropping a corner is invisible in practice, because a corner is
    /// drawn by *not* filling that part of the box rather than by painting over
    /// it.
    fn render_element_clipped<'a>(
        &self,
        element: &ComputedElement,
        gl_state: &RenderState,
        inherited_colors: Option<&ElementColors>,
        clip: Option<RectF>,
    ) -> anyhow::Result<()> {
        let clip = intersect_clip(clip, element.clip);
        if let Some(clip) = clip {
            if clip.width() <= 0. || clip.height() <= 0. {
                return Ok(());
            }
            if clip.intersection(&element.bounds).is_none() {
                return Ok(());
            }
        }

        let layer = gl_state.layer_for_zindex(element.zindex)?;
        let mut layers = layer.quad_allocator();

        let colors = match &element.hover_colors {
            Some(hc) => {
                let hovering =
                    match &self.current_mouse_event {
                        Some(event) => {
                            let mouse_x = event.coords.x as f32;
                            let mouse_y = event.coords.y as f32;
                            mouse_x >= element.bounds.min_x()
                                && mouse_x <= element.bounds.max_x()
                                && mouse_y >= element.bounds.min_y()
                                && mouse_y <= element.bounds.max_y()
                        }
                        None => false,
                    } && matches!(self.current_mouse_capture, None | Some(MouseCapture::UI));
                if hovering {
                    hc
                } else {
                    &element.colors
                }
            }
            None => &element.colors,
        };

        self.render_element_background(element, colors, &mut layers, inherited_colors, clip)?;
        let left = self.dimensions.pixel_width as f32 / -2.0;
        let top = self.dimensions.pixel_height as f32 / -2.0;
        match &element.content {
            ComputedElementContent::Text(cells) => {
                let mut pos_x = element.content_rect.min_x();
                for cell in cells {
                    if pos_x >= element.content_rect.max_x() {
                        break;
                    }
                    match cell {
                        ElementCell::Sprite(sprite) => {
                            let width = sprite.coords.width();
                            let height = sprite.coords.height();
                            let pos_y = element.content_rect.min_y();

                            if pos_x + width as f32 > element.content_rect.max_x() {
                                break;
                            }

                            let dest = euclid::rect(pos_x, pos_y, width as f32, height as f32);
                            if let Some((dest, tex)) =
                                clip_textured_quad(dest, sprite.texture_coords(), clip)
                            {
                                let mut quad = layers.allocate(2)?;
                                quad.set_position(
                                    dest.min_x() + left,
                                    dest.min_y() + top,
                                    dest.max_x() + left,
                                    dest.max_y() + top,
                                );
                                self.resolve_text(colors, inherited_colors).apply(&mut quad);
                                quad.set_texture(tex);
                                quad.set_hsv(None);
                            }
                            pos_x += width as f32;
                        }
                        ElementCell::Glyph(glyph) => {
                            if let Some(texture) = glyph.texture.as_ref() {
                                let pos_y = element.content_rect.min_y() as f32
                                    - (glyph.y_offset + glyph.bearing_y).get() as f32
                                    + element.baseline;

                                if pos_x + glyph.x_advance.get() as f32
                                    > element.content_rect.max_x()
                                {
                                    break;
                                }
                                let pos_x = pos_x + (glyph.x_offset + glyph.bearing_x).get() as f32;
                                let width = texture.coords.size.width as f32 * glyph.scale as f32;
                                let height = texture.coords.size.height as f32 * glyph.scale as f32;

                                let dest = euclid::rect(pos_x, pos_y, width, height);
                                if let Some((dest, tex)) =
                                    clip_textured_quad(dest, texture.texture_coords(), clip)
                                {
                                    let mut quad = layers.allocate(1)?;
                                    quad.set_position(
                                        dest.min_x() + left,
                                        dest.min_y() + top,
                                        dest.max_x() + left,
                                        dest.max_y() + top,
                                    );
                                    self.resolve_text(colors, inherited_colors).apply(&mut quad);
                                    quad.set_texture(tex);
                                    quad.set_has_color(glyph.has_color);
                                    quad.set_hsv(None);
                                }
                            }
                            pos_x += glyph.x_advance.get() as f32;
                        }
                    }
                }
            }
            ComputedElementContent::Children(kids) => {
                drop(layers);

                for kid in kids {
                    self.render_element_clipped(kid, gl_state, Some(colors), clip)?;
                }
            }
            ComputedElementContent::Poly { poly, line_width } => {
                if element.content_rect.width() >= poly.width {
                    // A poly is a single sprite whose shape carries the meaning,
                    // so trimming it would draw the wrong shape. Include it only
                    // when it fits wholly inside the clip.
                    let rect = euclid::Rect::new(
                        element.content_rect.origin,
                        euclid::size2(poly.width, poly.height),
                    );
                    if clip.map(|clip| clip.contains_rect(&rect)).unwrap_or(true) {
                        let mut quad = self.poly_quad(
                            &mut layers,
                            1,
                            element.content_rect.origin,
                            poly.poly,
                            *line_width,
                            euclid::size2(poly.width, poly.height),
                            LinearRgba::TRANSPARENT,
                        )?;
                        self.resolve_text(colors, inherited_colors).apply(&mut quad);
                    }
                }
            }
        }

        Ok(())
    }

    fn resolve_text(
        &self,
        colors: &ElementColors,
        inherited_colors: Option<&ElementColors>,
    ) -> ResolvedColor {
        match &colors.text {
            InheritableColor::Inherited => match inherited_colors {
                Some(colors) => self.resolve_text(colors, None),
                None => LinearRgba::TRANSPARENT.into(),
            },
            InheritableColor::Color(color) => (*color).into(),
            InheritableColor::Animated {
                color,
                alt_color,
                ease,
                one_shot,
            } => {
                if let Some((mix_value, next)) = ease.borrow_mut().intensity(*one_shot) {
                    self.update_next_frame_time(Some(next));
                    ResolvedColor {
                        color: *color,
                        alt_color: *alt_color,
                        mix_value,
                    }
                } else {
                    (*color).into()
                }
            }
        }
    }

    fn resolve_bg(
        &self,
        colors: &ElementColors,
        inherited_colors: Option<&ElementColors>,
    ) -> ResolvedColor {
        match &colors.bg {
            InheritableColor::Inherited => match inherited_colors {
                Some(colors) => self.resolve_bg(colors, None),
                None => LinearRgba::TRANSPARENT.into(),
            },
            InheritableColor::Color(color) => (*color).into(),
            InheritableColor::Animated {
                color,
                alt_color,
                ease,
                one_shot,
            } => {
                if let Some((mix_value, next)) = ease.borrow_mut().intensity(*one_shot) {
                    self.update_next_frame_time(Some(next));
                    ResolvedColor {
                        color: *color,
                        alt_color: *alt_color,
                        mix_value,
                    }
                } else {
                    (*color).into()
                }
            }
        }
    }

    /// `filled_rectangle`, trimmed to `clip`.
    ///
    /// A filled rectangle carries no texture detail -- it samples a solid
    /// sprite -- so trimming its geometry is exact rather than an
    /// approximation.
    fn filled_rectangle_clipped<'a>(
        &self,
        layers: &'a mut TripleLayerQuadAllocator,
        layer_num: usize,
        rect: RectF,
        color: LinearRgba,
        clip: Option<RectF>,
    ) -> anyhow::Result<Option<QuadImpl<'a>>> {
        let rect = match clip {
            Some(clip) => match clip.intersection(&rect) {
                Some(rect) => rect,
                None => return Ok(None),
            },
            None => rect,
        };
        if rect.width() <= 0. || rect.height() <= 0. {
            return Ok(None);
        }
        Ok(Some(self.filled_rectangle(layers, layer_num, rect, color)?))
    }

    /// `poly_quad`, included only when it fits wholly inside `clip`.
    ///
    /// See `render_element_clipped` for why dropping a straddling corner is
    /// invisible.
    fn poly_quad_clipped<'a>(
        &self,
        layers: &'a mut TripleLayerQuadAllocator,
        layer_num: usize,
        point: PointF,
        polys: &'static [Poly],
        underline_height: isize,
        cell_size: SizeF,
        color: LinearRgba,
        clip: Option<RectF>,
    ) -> anyhow::Result<Option<QuadImpl<'a>>> {
        if let Some(clip) = clip {
            if !clip.contains_rect(&euclid::Rect::new(point, cell_size)) {
                return Ok(None);
            }
        }
        Ok(Some(self.poly_quad(
            layers,
            layer_num,
            point,
            polys,
            underline_height,
            cell_size,
            color,
        )?))
    }

    fn render_element_background<'a>(
        &self,
        element: &ComputedElement,
        colors: &ElementColors,
        layers: &mut TripleLayerQuadAllocator,
        inherited_colors: Option<&ElementColors>,
        clip: Option<RectF>,
    ) -> anyhow::Result<()> {
        let mut top_left_width = 0.;
        let mut top_left_height = 0.;
        let mut top_right_width = 0.;
        let mut top_right_height = 0.;

        let mut bottom_left_width = 0.;
        let mut bottom_left_height = 0.;
        let mut bottom_right_width = 0.;
        let mut bottom_right_height = 0.;

        if let Some(c) = &element.border_corners {
            top_left_width = c.top_left.width;
            top_left_height = c.top_left.height;
            top_right_width = c.top_right.width;
            top_right_height = c.top_right.height;

            bottom_left_width = c.bottom_left.width;
            bottom_left_height = c.bottom_left.height;
            bottom_right_width = c.bottom_right.width;
            bottom_right_height = c.bottom_right.height;

            if top_left_width > 0. && top_left_height > 0. {
                if let Some(mut quad) = self.poly_quad_clipped(
                    layers,
                    0,
                    element.border_rect.origin,
                    c.top_left.poly,
                    element.border.top as isize,
                    euclid::size2(top_left_width, top_left_height),
                    colors.border.top,
                    clip,
                )? {
                    quad.set_grayscale();
                }
            }
            if top_right_width > 0. && top_right_height > 0. {
                if let Some(mut quad) = self.poly_quad_clipped(
                    layers,
                    0,
                    euclid::point2(
                        element.border_rect.max_x() - top_right_width,
                        element.border_rect.min_y(),
                    ),
                    c.top_right.poly,
                    element.border.top as isize,
                    euclid::size2(top_right_width, top_right_height),
                    colors.border.top,
                    clip,
                )? {
                    quad.set_grayscale();
                }
            }
            if bottom_left_width > 0. && bottom_left_height > 0. {
                if let Some(mut quad) = self.poly_quad_clipped(
                    layers,
                    0,
                    euclid::point2(
                        element.border_rect.min_x(),
                        element.border_rect.max_y() - bottom_left_height,
                    ),
                    c.bottom_left.poly,
                    element.border.bottom as isize,
                    euclid::size2(bottom_left_width, bottom_left_height),
                    colors.border.bottom,
                    clip,
                )? {
                    quad.set_grayscale();
                }
            }
            if bottom_right_width > 0. && bottom_right_height > 0. {
                if let Some(mut quad) = self.poly_quad_clipped(
                    layers,
                    0,
                    euclid::point2(
                        element.border_rect.max_x() - bottom_right_width,
                        element.border_rect.max_y() - bottom_right_height,
                    ),
                    c.bottom_right.poly,
                    element.border.bottom as isize,
                    euclid::size2(bottom_right_width, bottom_right_height),
                    colors.border.bottom,
                    clip,
                )? {
                    quad.set_grayscale();
                }
            }

            // Filling the background is more complex because we can't
            // simply fill the padding rect--we'd clobber the corner
            // graphics.
            // Instead, we consider the element as consisting of:
            //
            //   TL T TR
            //   L  C  R
            //   BL B BR
            //
            // We already rendered the corner pieces, so now we need
            // to do the rest

            // The `T` piece
            if let Some(mut quad) = self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + top_left_width,
                    element.border_rect.min_y(),
                    element.border_rect.width() - (top_left_width + top_right_width) as f32,
                    top_left_height.max(top_right_height),
                ),
                LinearRgba::TRANSPARENT,
                clip,
            )? {
                self.resolve_bg(colors, inherited_colors).apply(&mut quad);
            }

            // The `B` piece
            if let Some(mut quad) = self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + bottom_left_width,
                    element.border_rect.max_y() - bottom_left_height.max(bottom_right_height),
                    element.border_rect.width() - (bottom_left_width + bottom_right_width),
                    bottom_left_height.max(bottom_right_height),
                ),
                LinearRgba::TRANSPARENT,
                clip,
            )? {
                self.resolve_bg(colors, inherited_colors).apply(&mut quad);
            }

            // The `L` piece
            if let Some(mut quad) = self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x(),
                    element.border_rect.min_y() + top_left_height,
                    top_left_width.max(bottom_left_width),
                    element.border_rect.height() - (top_left_height + bottom_left_height),
                ),
                LinearRgba::TRANSPARENT,
                clip,
            )? {
                self.resolve_bg(colors, inherited_colors).apply(&mut quad);
            }

            // The `R` piece
            if let Some(mut quad) = self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.max_x() - top_right_width,
                    element.border_rect.min_y() + top_right_height,
                    top_right_width.max(bottom_right_width),
                    element.border_rect.height() - (top_right_height + bottom_right_height),
                ),
                LinearRgba::TRANSPARENT,
                clip,
            )? {
                self.resolve_bg(colors, inherited_colors).apply(&mut quad);
            }

            // The `C` piece
            if let Some(mut quad) = self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + top_left_width,
                    element.border_rect.min_y() + top_right_height.min(top_left_height),
                    element.border_rect.width() - (top_left_width + top_right_width),
                    element.border_rect.height()
                        - (top_right_height.min(top_left_height)
                            + bottom_right_height.min(bottom_left_height)),
                ),
                LinearRgba::TRANSPARENT,
                clip,
            )? {
                self.resolve_bg(colors, inherited_colors).apply(&mut quad);
            }
        } else if colors.bg != InheritableColor::Color(LinearRgba::TRANSPARENT) {
            if let Some(mut quad) = self.filled_rectangle_clipped(
                layers,
                0,
                element.padding,
                LinearRgba::TRANSPARENT,
                clip,
            )? {
                self.resolve_bg(colors, inherited_colors).apply(&mut quad);
            }
        }

        if element.border_rect == element.padding {
            // There's no border to be drawn
            return Ok(());
        }

        if element.border.top > 0. && colors.border.top != LinearRgba::TRANSPARENT {
            self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + top_left_width as f32,
                    element.border_rect.min_y(),
                    element.border_rect.width() - (top_left_width + top_right_width) as f32,
                    element.border.top,
                ),
                colors.border.top,
                clip,
            )?;
        }
        if element.border.bottom > 0. && colors.border.bottom != LinearRgba::TRANSPARENT {
            self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x() + bottom_left_width as f32,
                    element.border_rect.max_y() - element.border.bottom,
                    element.border_rect.width() - (bottom_left_width + bottom_right_width) as f32,
                    element.border.bottom,
                ),
                colors.border.bottom,
                clip,
            )?;
        }
        if element.border.left > 0. && colors.border.left != LinearRgba::TRANSPARENT {
            self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.min_x(),
                    element.border_rect.min_y() + top_left_height as f32,
                    element.border.left,
                    element.border_rect.height() - (top_left_height + bottom_left_height) as f32,
                ),
                colors.border.left,
                clip,
            )?;
        }
        if element.border.right > 0. && colors.border.right != LinearRgba::TRANSPARENT {
            self.filled_rectangle_clipped(
                layers,
                0,
                euclid::rect(
                    element.border_rect.max_x() - element.border.right,
                    element.border_rect.min_y() + top_right_height as f32,
                    element.border.left,
                    element.border_rect.height() - (top_right_height + bottom_right_height) as f32,
                ),
                colors.border.right,
                clip,
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn centring_splits_the_slack_and_keeps_every_pixel() {
        // An even split, and an odd one where the extra pixel must not vanish:
        // leading + trailing has to come back to the full slack, or the box
        // ends up narrower than the minimum it was laid out from.
        assert_eq!(center_slack(100., 140.), (20., 20.));
        let (leading, trailing) = center_slack(100., 141.);
        assert_eq!((leading, trailing), (20., 21.));
        assert_eq!(leading + trailing, 41.);
    }

    #[test]
    fn content_wider_than_the_minimum_gets_no_slack() {
        // Nothing to centre, and in particular nothing negative: a label longer
        // than its key must not be pulled left by a negative leading pad.
        assert_eq!(center_slack(200., 140.), (0., 0.));
        assert_eq!(center_slack(140., 140.), (0., 0.));
    }

    fn ui_item(bounds: RectF, clip: Option<RectF>) -> Vec<UIItem> {
        let element = ComputedElement {
            item_type: Some(UIItemType::AboveScrollThumb),
            zindex: 0,
            bounds,
            border_rect: bounds,
            border: PixelDimension::default(),
            border_corners: None,
            colors: ElementColors::default(),
            hover_colors: None,
            padding: bounds,
            content_rect: bounds,
            baseline: 0.,
            clip,
            content: ComputedElementContent::Children(vec![]),
        };
        element.ui_items()
    }

    #[test]
    fn a_clip_trims_the_hit_rectangle() {
        // A key that has panned half under the pinned/scrolling boundary at
        // x=200 must only answer taps in the half that is still visible.
        let items = ui_item(
            euclid::rect(150., 0., 100., 50.),
            Some(euclid::rect(200., 0., 300., 50.)),
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].x, 200);
        assert_eq!(items[0].width, 50);
    }

    #[test]
    fn a_fully_clipped_hit_rectangle_is_not_published() {
        // Not published as an empty rectangle either: hit_test uses inclusive
        // bounds, so a zero-sized rectangle still matches its own corner and
        // would steal the tap it was clipped away to avoid.
        let items = ui_item(
            euclid::rect(0., 0., 100., 50.),
            Some(euclid::rect(200., 0., 300., 50.)),
        );
        assert!(items.is_empty());
    }

    #[test]
    fn no_clip_leaves_the_hit_rectangle_alone() {
        let items = ui_item(euclid::rect(150., 10., 100., 50.), None);
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].x, items[0].y), (150, 10));
        assert_eq!((items[0].width, items[0].height), (100, 50));
    }

    #[test]
    fn clips_nest() {
        assert_eq!(
            intersect_clip(
                Some(euclid::rect(0., 0., 100., 100.)),
                Some(euclid::rect(50., 50., 100., 100.))
            ),
            Some(euclid::rect(50., 50., 50., 50.))
        );
        // Disjoint clips collapse to nothing rather than to None, which would
        // read as "unclipped".
        let empty = intersect_clip(
            Some(euclid::rect(0., 0., 10., 10.)),
            Some(euclid::rect(50., 50., 10., 10.)),
        )
        .unwrap();
        assert!(empty.width() == 0. || empty.height() == 0.);

        assert_eq!(
            intersect_clip(None, Some(euclid::rect(1., 2., 3., 4.))),
            Some(euclid::rect(1., 2., 3., 4.))
        );
        assert_eq!(intersect_clip(None, None), None);
    }

    #[test]
    fn a_clipped_quad_trims_position_and_texture_together() {
        let dest = euclid::rect(100., 0., 20., 10.);
        let tex = euclid::rect(0.5, 0.25, 0.1, 0.2);

        // The left half is cut away.
        let (dest, tex) =
            clip_textured_quad(dest, tex, Some(euclid::rect(110., 0., 100., 10.))).unwrap();
        assert_eq!(dest, euclid::rect(110., 0., 10., 10.));
        // Half the sprite's width, taken from its midpoint, and its full height.
        assert!((tex.min_x() - 0.55).abs() < 1e-6);
        assert!((tex.width() - 0.05).abs() < 1e-6);
        assert!((tex.min_y() - 0.25).abs() < 1e-6);
        assert!((tex.height() - 0.2).abs() < 1e-6);
    }

    #[test]
    fn an_unclipped_quad_is_untouched() {
        let dest = euclid::rect(100., 0., 20., 10.);
        let tex = euclid::rect(0.5, 0.25, 0.1, 0.2);
        assert_eq!(clip_textured_quad(dest, tex, None), Some((dest, tex)));
        // And so is one that fits entirely inside the clip.
        assert_eq!(
            clip_textured_quad(dest, tex, Some(euclid::rect(0., 0., 1000., 1000.))),
            Some((dest, tex))
        );
    }

    #[test]
    fn a_quad_outside_the_clip_is_dropped() {
        assert!(clip_textured_quad(
            euclid::rect(0., 0., 20., 10.),
            euclid::rect(0.5, 0.25, 0.1, 0.2),
            Some(euclid::rect(200., 0., 100., 10.))
        )
        .is_none());
    }
}
