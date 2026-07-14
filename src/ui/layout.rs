//! Responsive layout helpers: width/height tiers, the minimum-size guard, and
//! a centered-popup rectangle used by every overlay.

use ratatui::layout::Rect;

/// Width tier: W1 ≥110, W2 80–109, W3 60–79, W4 <60.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidthTier {
    W1,
    W2,
    W3,
    W4,
}

/// Height tier: H1 ≥30, H2 20–29, H3 15–19, H4 <15.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightTier {
    H1,
    H2,
    H3,
    H4,
}

/// The width and height tiers for a given draw area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Breakpoints {
    pub w: WidthTier,
    pub h: HeightTier,
}

impl Breakpoints {
    pub fn of(area: Rect) -> Self {
        let w = if area.width >= 110 {
            WidthTier::W1
        } else if area.width >= 80 {
            WidthTier::W2
        } else if area.width >= 60 {
            WidthTier::W3
        } else {
            WidthTier::W4
        };
        let h = if area.height >= 30 {
            HeightTier::H1
        } else if area.height >= 20 {
            HeightTier::H2
        } else if area.height >= 15 {
            HeightTier::H3
        } else {
            HeightTier::H4
        };
        Self { w, h }
    }
}

/// Minimum usable terminal size. Below this the min-size guard takes over.
pub const MIN_W: u16 = 40;
pub const MIN_H: u16 = 10;

/// True when the area is too small to render the normal UI.
pub fn too_small(area: Rect) -> bool {
    area.width < MIN_W || area.height < MIN_H
}

/// A centered rectangle of the desired size, clamped to leave at least a
/// one-cell margin on every side. All arithmetic saturates.
pub fn popup_rect(desired_w: u16, desired_h: u16, area: Rect) -> Rect {
    let max_w = area.width.saturating_sub(2).max(1);
    let max_h = area.height.saturating_sub(2).max(1);
    let w = desired_w.clamp(1, max_w);
    let h = desired_h.clamp(1, max_h);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}
