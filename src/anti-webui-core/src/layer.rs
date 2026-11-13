//! What a layer surface of a desktop shell is: plain data the platform turns into
//! `zwlr_layer_shell_v1` requests (see `docs/design/37-wayland-shell.md`). Programs build it with
//! `anti-webui-wayland-shell`, which checks it first.

use crate::geometry::RectI;

/// Which of the compositor's layers a surface is on, from the bottom up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Layer {
    /// Under everything, like a wallpaper.
    Background,
    /// Under windows.
    Bottom,
    /// Over windows, like a bar.
    #[default]
    Top,
    /// Over everything, even windows on all of the screen.
    Overlay,
}

/// Anchor of the screen, which can be combined with `|`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Anchor(u8);

impl Anchor {
    pub const NONE: Anchor = Anchor(0);
    pub const TOP: Anchor = Anchor(1);
    pub const BOTTOM: Anchor = Anchor(2);
    pub const LEFT: Anchor = Anchor(4);
    pub const RIGHT: Anchor = Anchor(8);
    pub const ALL: Anchor = Anchor(15);

    pub fn contains(self, other: Anchor) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    /// Whether both the left and the right edge are in.
    pub fn spans_width(self) -> bool {
        self.contains(Anchor::LEFT | Anchor::RIGHT)
    }

    /// Whether both the top and the bottom edge are in.
    pub fn spans_height(self) -> bool {
        self.contains(Anchor::TOP | Anchor::BOTTOM)
    }

    /// The one edge a surface keeps other surfaces away from: anchored to it alone, or to it and both
    /// edges next to it.
    pub fn exclusive_edge(self) -> Option<Anchor> {
        let (t, b, l, r) = (
            self.contains(Anchor::TOP),
            self.contains(Anchor::BOTTOM),
            self.contains(Anchor::LEFT),
            self.contains(Anchor::RIGHT),
        );
        match (t, b, l, r) {
            (true, false, false, false) | (true, false, true, true) => Some(Anchor::TOP),
            (false, true, false, false) | (false, true, true, true) => Some(Anchor::BOTTOM),
            (false, false, true, false) | (true, true, true, false) => Some(Anchor::LEFT),
            (false, false, false, true) | (true, true, false, true) => Some(Anchor::RIGHT),
            _ => None,
        }
    }
}

impl std::ops::BitOr for Anchor {
    type Output = Anchor;
    fn bitor(self, rhs: Anchor) -> Anchor {
        Anchor(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Anchor {
    fn bitor_assign(&mut self, rhs: Anchor) {
        self.0 |= rhs.0;
    }
}

/// How big a surface is along one axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LayerSize {
    /// Logical pixels.
    Fixed(i32),
    /// From edge to edge; only when anchored to both.
    Stretch,
    /// The max-content size of what is in it, following it as it changes.
    #[default]
    Content,
}

/// How a surface keeps other surfaces and windows away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Exclusive {
    /// It doesn't, and it moves out of the way of those that do.
    #[default]
    None,
    /// By its own size toward the edge it is anchored to, like a bar.
    Auto,
    /// By this many logical pixels.
    Fixed(i32),
    /// It doesn't, and it doesn't move out of the way either.
    Ignore,
}

/// Whether a surface gets the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Keyboard {
    #[default]
    None,
    /// When it is clicked, like a window.
    OnDemand,
    /// All of it, while the surface is there and on a layer that allows it.
    Exclusive,
}

/// Where a surface takes the pointer; elsewhere it goes to what is below.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum InputRegion {
    #[default]
    Full,
    /// Nowhere: every click goes through.
    Empty,
    /// These rectangles, in logical pixels.
    Rects(Vec<RectI>),
}

/// Space between a surface and the edges it is anchored to, in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Margin {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl Margin {
    pub fn all(v: i32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
}

/// A layer surface, as it is asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct LayerSpec {
    pub layer: Layer,
    pub anchor: Anchor,
    pub width: LayerSize,
    pub height: LayerSize,
    pub exclusive: Exclusive,
    /// For a surface anchored to a corner: the edge [`Exclusive`] applies to.
    pub exclusive_edge: Option<Anchor>,
    pub margin: Margin,
    pub keyboard: Keyboard,
    pub input_region: InputRegion,
    /// The connector name of the output, like `DP-1`; `None` leaves it to the compositor.
    pub output: Option<String>,
    pub namespace: String,
}

impl LayerSpec {
    /// Whether one axis or both follow what is in it.
    pub fn fits_content(&self) -> (bool, bool) {
        (
            self.width == LayerSize::Content,
            self.height == LayerSize::Content,
        )
    }

    /// The exclusive zone to ask for. Fixed axes use the requested size (at least one pixel),
    /// even before configure; content and stretched axes use the supplied surface size.
    pub fn exclusive_zone(&self, width: i32, height: i32) -> i32 {
        match self.exclusive {
            Exclusive::None => 0,
            Exclusive::Ignore => -1,
            Exclusive::Fixed(n) => n.max(0),
            Exclusive::Auto => {
                let along = |size, current| match size {
                    LayerSize::Fixed(n) => n.max(1),
                    LayerSize::Content | LayerSize::Stretch => current,
                };
                let edge = self.exclusive_edge.or(self.anchor.exclusive_edge());
                match edge {
                    Some(e) if e == Anchor::TOP || e == Anchor::BOTTOM => {
                        along(self.height, height)
                    }
                    Some(e) if e == Anchor::LEFT || e == Anchor::RIGHT => along(self.width, width),
                    _ => 0,
                }
            }
        }
    }
}

/// What changes on a layer surface after it opened.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerChange {
    Size(LayerSize, LayerSize),
    /// Change anchors and both size policies in one surface commit.
    Geometry {
        anchor: Anchor,
        width: LayerSize,
        height: LayerSize,
    },
    Anchor(Anchor),
    Margin(Margin),
    Exclusive(Exclusive),
    Keyboard(Keyboard),
    Layer(Layer),
    InputRegion(InputRegion),
}

impl LayerSpec {
    pub fn apply(&mut self, change: &LayerChange) {
        match change.clone() {
            LayerChange::Size(w, h) => {
                self.width = w;
                self.height = h;
            }
            LayerChange::Anchor(a) => self.anchor = a,
            LayerChange::Geometry {
                anchor,
                width,
                height,
            } => {
                self.anchor = anchor;
                self.width = width;
                self.height = height;
            }
            LayerChange::Margin(m) => self.margin = m,
            LayerChange::Exclusive(e) => self.exclusive = e,
            LayerChange::Keyboard(k) => self.keyboard = k,
            LayerChange::Layer(l) => self.layer = l,
            LayerChange::InputRegion(r) => self.input_region = r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edge_kept_clear_is_the_one_anchored_alone_or_with_both_neighbours() {
        let bar = Anchor::TOP | Anchor::LEFT | Anchor::RIGHT;
        assert_eq!(bar.exclusive_edge(), Some(Anchor::TOP));
        assert_eq!(Anchor::LEFT.exclusive_edge(), Some(Anchor::LEFT));
        let dock = Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
        assert_eq!(dock.exclusive_edge(), Some(Anchor::BOTTOM));
        assert_eq!((Anchor::TOP | Anchor::LEFT).exclusive_edge(), None);
        assert_eq!(Anchor::ALL.exclusive_edge(), None);
    }

    #[test]
    fn an_automatic_zone_is_the_size_toward_that_edge() {
        let spec = LayerSpec {
            anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            exclusive: Exclusive::Auto,
            ..Default::default()
        };
        assert_eq!(spec.exclusive_zone(1920, 32), 32);
        let side = LayerSpec {
            anchor: Anchor::RIGHT | Anchor::TOP | Anchor::BOTTOM,
            ..spec.clone()
        };
        assert_eq!(side.exclusive_zone(48, 1080), 48);
        let corner = LayerSpec {
            anchor: Anchor::TOP | Anchor::RIGHT,
            ..spec.clone()
        };
        assert_eq!(corner.exclusive_zone(300, 100), 0);
        let told = LayerSpec {
            exclusive_edge: Some(Anchor::RIGHT),
            ..corner
        };
        assert_eq!(told.exclusive_zone(300, 100), 300);
    }
}
