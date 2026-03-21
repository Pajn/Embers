#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl PtySize {
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub fn contains(&self, point: Point) -> bool {
        let max_x = self.origin.x + i32::from(self.size.width);
        let max_y = self.origin.y + i32::from(self.size.height);

        point.x >= self.origin.x && point.y >= self.origin.y && point.x < max_x && point.y < max_y
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FloatGeometry {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl FloatGeometry {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl std::fmt::Display for SplitDirection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        };

        formatter.write_str(label)
    }
}

#[cfg(test)]
mod tests {
    use super::{Point, Rect, Size};

    #[test]
    fn rect_contains_only_points_inside_bounds() {
        let rect = Rect {
            origin: Point { x: 10, y: 20 },
            size: Size {
                width: 4,
                height: 3,
            },
        };

        assert!(rect.contains(Point { x: 10, y: 20 }));
        assert!(rect.contains(Point { x: 13, y: 22 }));
        assert!(!rect.contains(Point { x: 14, y: 22 }));
        assert!(!rect.contains(Point { x: 13, y: 23 }));
    }
}
