use mux_core::Rect;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderGrid {
    width: u16,
    height: u16,
    cells: Vec<char>,
}

impl RenderGrid {
    pub fn new(width: u16, height: u16) -> Self {
        let len = usize::from(width) * usize::from(height);
        Self {
            width,
            height,
            cells: vec![' '; len],
        }
    }

    pub fn clear(&mut self, fill: char) {
        self.cells.fill(fill);
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn put_char(&mut self, x: u16, y: u16, ch: char) {
        if x >= self.width || y >= self.height {
            return;
        }

        let idx = usize::from(y) * usize::from(self.width) + usize::from(x);
        self.cells[idx] = ch;
    }

    pub fn put_str(&mut self, x: u16, y: u16, text: &str) {
        if y >= self.height {
            return;
        }

        for (offset, ch) in text.chars().enumerate() {
            let Some(x_pos) = x.checked_add(offset as u16) else {
                break;
            };
            if x_pos >= self.width {
                break;
            }
            self.put_char(x_pos, y, ch);
        }
    }

    pub fn draw_hline(&mut self, x: u16, y: u16, width: u16, ch: char) {
        for offset in 0..width {
            self.put_char(x.saturating_add(offset), y, ch);
        }
    }

    pub fn draw_vline(&mut self, x: u16, y: u16, height: u16, ch: char) {
        for offset in 0..height {
            self.put_char(x, y.saturating_add(offset), ch);
        }
    }

    pub fn draw_box(&mut self, rect: Rect, border: BorderStyle) {
        if rect.size.width == 0 || rect.size.height == 0 {
            return;
        }

        let x = rect.origin.x.max(0) as u16;
        let y = rect.origin.y.max(0) as u16;
        let width = rect.size.width;
        let height = rect.size.height;
        let right = x.saturating_add(width.saturating_sub(1));
        let bottom = y.saturating_add(height.saturating_sub(1));

        self.put_char(x, y, border.top_left);
        self.put_char(right, y, border.top_right);
        self.put_char(x, bottom, border.bottom_left);
        self.put_char(right, bottom, border.bottom_right);

        if width > 2 {
            self.draw_hline(x + 1, y, width - 2, border.horizontal);
            self.draw_hline(x + 1, bottom, width - 2, border.horizontal);
        }

        if height > 2 {
            self.draw_vline(x, y + 1, height - 2, border.vertical);
            self.draw_vline(right, y + 1, height - 2, border.vertical);
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.cells
            .chunks(usize::from(self.width))
            .map(|row| row.iter().collect::<String>())
            .collect()
    }

    pub fn render(&self) -> String {
        self.lines().join("\n")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BorderStyle {
    pub top_left: char,
    pub top_right: char,
    pub bottom_left: char,
    pub bottom_right: char,
    pub horizontal: char,
    pub vertical: char,
}

impl BorderStyle {
    pub const ASCII: Self = Self {
        top_left: '+',
        top_right: '+',
        bottom_left: '+',
        bottom_right: '+',
        horizontal: '-',
        vertical: '|',
    };

    pub const FOCUSED: Self = Self {
        top_left: '#',
        top_right: '#',
        bottom_left: '#',
        bottom_right: '#',
        horizontal: '#',
        vertical: '#',
    };
}
