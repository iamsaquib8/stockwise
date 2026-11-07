use colored::Colorize;

// ── Braille Line Chart ──
// Each braille char = 2 wide x 4 tall pixel grid
// Dot positions:  1 4    Values: 1  8
//                 2 5            2  16
//                 3 6            4  32
//                 7 8            64 128

const BRAILLE_BASE: u32 = 0x2800;
const DOT_MAP: [[u32; 2]; 4] = [
    [0x01, 0x08], // row 0
    [0x02, 0x10], // row 1
    [0x04, 0x20], // row 2
    [0x40, 0x80], // row 3
];

/// Shown instead of a chart when the terminal is too narrow or short to draw one.
const TOO_SMALL: &str = "  Chart dimensions too small";
/// Columns one candlestick takes: the candle itself plus a gap.
const CANDLE_WIDTH: usize = 2;

struct BrailleCanvas {
    width: usize,  // in chars
    height: usize, // in chars
    cells: Vec<u32>,
}

impl BrailleCanvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0; width * height],
        }
    }

    /// Set a pixel at (x, y) where x is in [0, width*2) and y is in [0, height*4)
    fn set(&mut self, x: usize, y: usize) {
        let cx = x / 2;
        let cy = y / 4;
        if cx >= self.width || cy >= self.height {
            return;
        }
        let px = x % 2;
        let py = y % 4;
        self.cells[cy * self.width + cx] |= DOT_MAP[py][px];
    }

    /// Draw a line between two pixel coordinates using Bresenham's
    fn line(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        let (mut x0, mut y0) = (x0 as i32, y0 as i32);
        let (x1, y1) = (x1 as i32, y1 as i32);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            if x0 >= 0 && y0 >= 0 {
                self.set(x0 as usize, y0 as usize);
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    fn render(&self) -> Vec<String> {
        let mut rows = Vec::new();
        for y in 0..self.height {
            let mut row = String::new();
            for x in 0..self.width {
                let code = BRAILLE_BASE + self.cells[y * self.width + x];
                row.push(char::from_u32(code).unwrap_or(' '));
            }
            rows.push(row);
        }
        rows
    }
}

/// Render a braille line chart with axis labels
/// Returns a vector of lines ready to print
pub fn line_chart(
    data: &[f64],
    width: usize,
    height: usize,
    color: &str,
    title: &str,
) -> Vec<String> {
    if data.is_empty() {
        return vec!["  No data".to_string()];
    }
    if width == 0 || height == 0 {
        return vec![TOO_SMALL.to_string()];
    }

    let min = data.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = data.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = if (max - min).abs() < f64::EPSILON {
        1.0
    } else {
        max - min
    };

    let px_w = width * 2;
    let px_h = height * 4;

    let mut canvas = BrailleCanvas::new(width, height);

    // Map data to pixel coordinates
    let points: Vec<(usize, usize)> = data
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let x = if data.len() > 1 {
                (i as f64 / (data.len() - 1) as f64 * (px_w - 1) as f64) as usize
            } else {
                px_w / 2
            };
            // y is inverted (0=top)
            let y = ((1.0 - (v - min) / range) * (px_h - 1) as f64) as usize;
            (x, y.min(px_h - 1))
        })
        .collect();

    // Draw lines between consecutive points
    for pair in points.windows(2) {
        canvas.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1);
    }

    let chart_lines = canvas.render();
    let mut output = Vec::new();

    // Title
    if !title.is_empty() {
        output.push(format!("  {}", title.bold()));
        output.push(String::new());
    }

    // Y-axis labels + chart
    let label_width = 10;
    for (i, line) in chart_lines.iter().enumerate() {
        let label = if i == 0 {
            format!("{:>8.2}", max)
        } else if i == height - 1 {
            format!("{:>8.2}", min)
        } else if i == height / 2 {
            format!("{:>8.2}", (max + min) / 2.0)
        } else {
            " ".repeat(8)
        };

        let colored_line = match color {
            "green" => line.green().to_string(),
            "red" => line.red().to_string(),
            "cyan" => line.cyan().to_string(),
            "yellow" => line.yellow().to_string(),
            _ => line.to_string(),
        };

        output.push(format!(
            "  {} {}{}",
            label.dimmed(),
            "│".dimmed(),
            colored_line
        ));
    }

    // X-axis
    output.push(
        format!(
            "  {} {}",
            " ".repeat(label_width - 2),
            "└".to_string() + &"─".repeat(width)
        )
        .dimmed()
        .to_string(),
    );

    output
}

/// Render a dual-line chart (e.g., price vs moving average)
pub fn dual_line_chart(
    data1: &[f64],
    data2: &[f64],
    width: usize,
    height: usize,
    title: &str,
    label1: &str,
    label2: &str,
) -> Vec<String> {
    if data1.is_empty() {
        return vec!["  No data".to_string()];
    }
    if width == 0 || height == 0 {
        return vec![TOO_SMALL.to_string()];
    }

    let all_min = data1
        .iter()
        .chain(data2.iter())
        .cloned()
        .fold(f64::INFINITY, f64::min);
    let all_max = data1
        .iter()
        .chain(data2.iter())
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let range = if (all_max - all_min).abs() < f64::EPSILON {
        1.0
    } else {
        all_max - all_min
    };

    let px_w = width * 2;
    let px_h = height * 4;

    let mut canvas1 = BrailleCanvas::new(width, height);
    let mut canvas2 = BrailleCanvas::new(width, height);

    let map_points = |data: &[f64]| -> Vec<(usize, usize)> {
        data.iter()
            .enumerate()
            .map(|(i, &v)| {
                let x = if data.len() > 1 {
                    (i as f64 / (data.len() - 1) as f64 * (px_w - 1) as f64) as usize
                } else {
                    px_w / 2
                };
                let y = ((1.0 - (v - all_min) / range) * (px_h - 1) as f64) as usize;
                (x, y.min(px_h - 1))
            })
            .collect()
    };

    let pts1 = map_points(data1);
    let pts2 = map_points(data2);

    for pair in pts1.windows(2) {
        canvas1.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1);
    }
    for pair in pts2.windows(2) {
        canvas2.line(pair[0].0, pair[0].1, pair[1].0, pair[1].1);
    }

    let lines1 = canvas1.render();
    let lines2 = canvas2.render();

    let mut output = Vec::new();
    if !title.is_empty() {
        output.push(format!("  {}", title.bold()));
        output.push(format!(
            "  {} {}  {} {}",
            "━".green(),
            label1.green(),
            "━".yellow(),
            label2.yellow()
        ));
        output.push(String::new());
    }

    for (i, (l1, l2)) in lines1.iter().zip(lines2.iter()).enumerate() {
        let label = if i == 0 {
            format!("{:>8.2}", all_max)
        } else if i == height - 1 {
            format!("{:>8.2}", all_min)
        } else if i == height / 2 {
            format!("{:>8.2}", (all_max + all_min) / 2.0)
        } else {
            " ".repeat(8)
        };

        // Merge: overlay canvas2 chars onto canvas1 using color
        let mut merged = String::new();
        for (c1, c2) in l1.chars().zip(l2.chars()) {
            let b1 = c1 as u32 - BRAILLE_BASE;
            let b2 = c2 as u32 - BRAILLE_BASE;
            if b1 != 0 && b2 != 0 {
                // Both have data — combine and show in cyan
                let combined = char::from_u32(BRAILLE_BASE + (b1 | b2)).unwrap_or(' ');
                merged.push_str(&combined.to_string().cyan().to_string());
            } else if b1 != 0 {
                merged.push_str(&c1.to_string().green().to_string());
            } else if b2 != 0 {
                merged.push_str(&c2.to_string().yellow().to_string());
            } else {
                merged.push(' ');
            }
        }

        output.push(format!("  {} {}{}", label.dimmed(), "│".dimmed(), merged));
    }

    output.push(
        format!(
            "  {}  {}",
            " ".repeat(8),
            "└".to_string() + &"─".repeat(width)
        )
        .dimmed()
        .to_string(),
    );

    output
}

/// Horizontal bar chart (for sectors, comparison, etc.)
pub fn bar_chart(items: &[(&str, f64, bool)], width: usize) -> Vec<String> {
    if items.is_empty() {
        return vec![];
    }

    let max_abs = items
        .iter()
        .map(|(_, v, _)| v.abs())
        .fold(0.0_f64, f64::max);
    if max_abs == 0.0 {
        return vec![];
    }

    let mut output = Vec::new();
    for (label, value, positive) in items {
        let bar_len = ((value.abs() / max_abs) * width as f64) as usize;
        let bar = "█".repeat(bar_len.max(1));
        let colored_bar = if *positive {
            bar.green().to_string()
        } else {
            bar.red().to_string()
        };
        let val_str = if *positive {
            format!("{:+.2}%", value).green().to_string()
        } else {
            format!("{:+.2}%", value).red().to_string()
        };

        output.push(format!(
            "  {:<16} {} {}",
            label.bold(),
            colored_bar,
            val_str
        ));
    }
    output
}

/// Volume bar chart (vertical-ish using Unicode blocks)
pub fn volume_bars(volumes: &[u64], width: usize, height: usize) -> Vec<String> {
    if volumes.is_empty() {
        return vec![];
    }

    let max_vol = (*volumes.iter().max().unwrap_or(&1)).max(1) as f64;
    // Partial blocks in eighths; index 0 = empty.
    let blocks = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    // Downsample (bucket-average) to at most `width` columns; never pad with
    // empty columns when there is less data than width.
    let cols = width.min(volumes.len()).max(1);
    let step = volumes.len() as f64 / cols as f64;
    let sampled: Vec<u64> = (0..cols)
        .map(|i| {
            let start = (i as f64 * step) as usize;
            let end = (((i + 1) as f64 * step) as usize).clamp(start + 1, volumes.len());
            volumes[start..end].iter().sum::<u64>() / (end - start) as u64
        })
        .collect();

    // Each column is a vertical bar `height` cells tall at max volume. Row r
    // (counted from the bottom) shows the part of the bar between r and r+1.
    let mut rows = Vec::new();
    for row in (0..height).rev() {
        let line: String = sampled
            .iter()
            .map(|&vol| {
                let bar = vol as f64 / max_vol * height as f64;
                let fill = (bar - row as f64).clamp(0.0, 1.0);
                blocks[(fill * 8.0).round() as usize]
            })
            .collect();
        rows.push(format!("  {}", line.cyan()));
    }
    rows
}

/// OHLC Candlestick chart
pub fn candlestick_chart(
    opens: &[f64],
    highs: &[f64],
    lows: &[f64],
    closes: &[f64],
    width: usize,
    height: usize,
) -> Vec<String> {
    let n = opens
        .len()
        .min(highs.len())
        .min(lows.len())
        .min(closes.len());
    if n == 0 {
        return vec!["  No data".to_string()];
    }
    // One candle takes `CANDLE_WIDTH` columns (the body and a gap).
    if width < CANDLE_WIDTH || height == 0 {
        return vec![TOO_SMALL.to_string()];
    }

    let all_min = lows.iter().take(n).cloned().fold(f64::INFINITY, f64::min);
    let all_max = highs
        .iter()
        .take(n)
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let range = if (all_max - all_min).abs() < f64::EPSILON {
        1.0
    } else {
        all_max - all_min
    };

    // Map price to row (0=top=max, height-1=bottom=min)
    let to_row = |price: f64| -> usize {
        let r = ((1.0 - (price - all_min) / range) * (height - 1) as f64) as usize;
        r.min(height - 1)
    };

    // Build grid
    let mut grid = vec![vec![' '; width]; height];

    // Downsample candles to fit width (each candle = 1 char wide with 1 space)
    let candle_width = CANDLE_WIDTH;
    let max_candles = width / candle_width;
    let step = if n > max_candles {
        n as f64 / max_candles as f64
    } else {
        1.0
    };
    let num_candles = (n as f64 / step).ceil() as usize;

    for i in 0..num_candles.min(max_candles) {
        let idx = (i as f64 * step) as usize;
        if idx >= n {
            break;
        }

        let col = i * candle_width;
        if col >= width {
            break;
        }

        let open = opens[idx];
        let high = highs[idx];
        let low = lows[idx];
        let close = closes[idx];

        let bullish = close >= open;
        let body_top = to_row(close.max(open));
        let body_bot = to_row(close.min(open));
        let wick_top = to_row(high);
        let wick_bot = to_row(low);

        // Draw wick
        for row in wick_top..=wick_bot {
            if row < height {
                grid[row][col] = '│';
            }
        }

        // Draw body (overwrite wick)
        let body_char = if bullish { '┃' } else { '┃' };
        for row in body_top..=body_bot {
            if row < height {
                grid[row][col] = body_char;
            }
        }
        // If body is single row
        if body_top == body_bot && body_top < height {
            grid[body_top][col] = if bullish { '─' } else { '─' };
        }
    }

    let mut output = Vec::new();
    output.push(format!("  {}", "Candlestick Chart".bold()));
    output.push(String::new());

    for (i, row) in grid.iter().enumerate() {
        let label = if i == 0 {
            format!("{:>8.2}", all_max)
        } else if i == height - 1 {
            format!("{:>8.2}", all_min)
        } else if i == height / 2 {
            format!("{:>8.2}", (all_max + all_min) / 2.0)
        } else {
            " ".repeat(8)
        };

        // Color based on candle direction
        let mut colored_row = String::new();
        for (col_idx, &ch) in row.iter().enumerate() {
            if ch == '│' || ch == '┃' || ch == '─' {
                // Determine if this column's candle was bullish
                let candle_idx = col_idx / 2;
                let data_idx = (candle_idx as f64 * step) as usize;
                let bullish = data_idx < n && closes[data_idx] >= opens[data_idx];
                if bullish {
                    colored_row.push_str(&ch.to_string().green().to_string());
                } else {
                    colored_row.push_str(&ch.to_string().red().to_string());
                }
            } else {
                colored_row.push(ch);
            }
        }

        output.push(format!(
            "  {} {}{}",
            label.dimmed(),
            "│".dimmed(),
            colored_row
        ));
    }

    output.push(
        format!(
            "  {}  {}",
            " ".repeat(8),
            "└".to_string() + &"─".repeat(width)
        )
        .dimmed()
        .to_string(),
    );

    output
}

/// RSI gauge visualization
pub fn rsi_gauge(rsi: f64) -> String {
    let total = 40;
    let pos = ((rsi / 100.0) * total as f64) as usize;
    let pos = pos.min(total);

    let mut bar = String::new();
    for i in 0..total {
        if i == pos {
            bar.push_str(&"▼".bold().to_string());
        } else if i < 12 {
            // 0-30 oversold zone
            bar.push_str(&"░".green().to_string());
        } else if i > 28 {
            // 70-100 overbought zone
            bar.push_str(&"░".red().to_string());
        } else {
            bar.push_str(&"░".dimmed().to_string());
        }
    }

    format!("  {}  {}  {}", "0".dimmed(), bar, "100".dimmed())
}

/// Portfolio allocation donut/bar
/// Portfolio allocation: one bar per holding, heaviest first, so rows stay
/// readable without colour and with more holdings than the old palette.
pub fn allocation_chart(items: &[(&str, f64)]) -> Vec<String> {
    let total: f64 = items.iter().map(|(_, v)| v).sum();
    if total == 0.0 {
        return vec![];
    }

    let bar_width = 40;
    let label_width = items
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0)
        .max(12);

    let mut rows: Vec<(&str, f64)> = items.to_vec();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));

    rows.iter()
        .map(|&(label, value)| {
            let pct = value / total;
            let filled = (pct * bar_width as f64).round() as usize;
            format!(
                "  {:<label_width$} {:>5.1}% {}",
                label,
                pct * 100.0,
                "█".repeat(filled)
            )
        })
        .collect()
}

/// Option payoff chart: profit/loss against the underlying's price. The
/// expiry curve is green above zero and red below; the optional
/// before-expiry curve is yellow; a dotted zero line and a dotted line at
/// `spot` are dimmed. `prices` are ascending and line up with the curves.
pub fn payoff_chart(
    prices: &[f64],
    at_expiry: &[f64],
    today: Option<&[f64]>,
    spot: f64,
    width: usize,
    height: usize,
) -> Vec<String> {
    if prices.len() < 2 || at_expiry.len() != prices.len() || width < 2 || height < 2 {
        return vec!["  No data".to_string()];
    }
    let today = today.filter(|t| t.len() == prices.len());
    let finite = |v: &&f64| v.is_finite();
    let values = at_expiry
        .iter()
        .chain(today.into_iter().flatten())
        .filter(finite);
    // Always show the zero line.
    let (lo, hi) = values.fold((0.0f64, 0.0f64), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let (lo, hi) = if (hi - lo).abs() < f64::EPSILON {
        (lo - 1.0, hi + 1.0)
    } else {
        (lo, hi)
    };
    let (px_w, px_h) = (width * 2, height * 4);
    let (x0, x1) = (prices[0], prices[prices.len() - 1]);
    let to_px = |i: usize, v: f64| -> (usize, usize) {
        let x = (i as f64 / (prices.len() - 1) as f64 * (px_w - 1) as f64).round() as usize;
        let y = ((hi - v) / (hi - lo) * (px_h - 1) as f64).round() as usize;
        (x, y.min(px_h - 1))
    };
    let zero_y = to_px(0, 0.0).1;

    let draw = |curve: &[f64]| {
        let mut c = BrailleCanvas::new(width, height);
        let pts: Vec<(usize, usize)> = curve
            .iter()
            .enumerate()
            .map(|(i, v)| to_px(i, *v))
            .collect();
        for p in pts.windows(2) {
            c.line(p[0].0, p[0].1, p[1].0, p[1].1);
        }
        c
    };
    let expiry = draw(at_expiry);
    let before = today.map(draw);
    let mut axes = BrailleCanvas::new(width, height);
    for x in (0..px_w).step_by(2) {
        axes.set(x, zero_y);
    }
    if x1 > x0 && (x0..=x1).contains(&spot) {
        let sx = ((spot - x0) / (x1 - x0) * (px_w - 1) as f64).round() as usize;
        for y in (0..px_h).step_by(2) {
            axes.set(sx, y);
        }
    }

    // Colour each column of the expiry curve by the payoff's sign there.
    let column_value = |cx: usize| {
        let i = ((cx * 2) as f64 / (px_w - 1) as f64 * (prices.len() - 1) as f64).round() as usize;
        at_expiry[i.min(prices.len() - 1)]
    };
    let label = |v: f64| {
        if v == 0.0 {
            format!("{:>10}", 0)
        } else if v.abs() >= 100.0 {
            format!("{:>10.0}", v)
        } else {
            format!("{:>10.2}", v)
        }
    };
    let zero_row = zero_y / 4;
    let mut out = Vec::with_capacity(height + 2);
    for row in 0..height {
        let mut line = String::new();
        for cx in 0..width {
            let i = row * width + cx;
            let (e, a) = (expiry.cells[i], axes.cells[i]);
            let b = before.as_ref().map_or(0, |c| c.cells[i]);
            let glyph = |bits: u32| {
                char::from_u32(BRAILLE_BASE + bits)
                    .unwrap_or(' ')
                    .to_string()
            };
            if e != 0 {
                let g = glyph(e | b);
                line.push_str(&if column_value(cx) >= 0.0 {
                    g.green().to_string()
                } else {
                    g.red().to_string()
                });
            } else if b != 0 {
                line.push_str(&glyph(b).yellow().to_string());
            } else if a != 0 {
                line.push_str(&glyph(a).dimmed().to_string());
            } else {
                line.push(' ');
            }
        }
        let tick = if row == 0 {
            label(hi)
        } else if row == height - 1 {
            label(lo)
        } else if row == zero_row {
            label(0.0)
        } else {
            " ".repeat(10)
        };
        out.push(format!("  {} {}{}", tick.dimmed(), "│".dimmed(), line));
    }
    out.push(
        format!("  {} └{}", " ".repeat(10), "─".repeat(width))
            .dimmed()
            .to_string(),
    );
    let (left, right) = (format!("{:.0}", x0), format!("{:.0}", x1));
    let mut axis = vec![' '; width + 1];
    let mut put = |at: usize, text: &str| {
        for (k, ch) in text.chars().enumerate() {
            if let Some(slot) = axis.get_mut(at + k) {
                *slot = ch;
            }
        }
    };
    put(0, &left);
    put((width + 1).saturating_sub(right.len()), &right);
    if x1 > x0 && (x0..=x1).contains(&spot) {
        let s = format!("▲{:.0}", spot);
        let at = ((spot - x0) / (x1 - x0) * width as f64).round() as usize;
        let at = at.clamp(
            left.len() + 1,
            (width + 1)
                .saturating_sub(right.len() + s.chars().count() + 1)
                .max(left.len() + 1),
        );
        put(at, &s);
    }
    out.push(
        format!("  {}  {}", " ".repeat(10), axis.iter().collect::<String>())
            .dimmed()
            .to_string(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_chart_empty() {
        let result = line_chart(&[], 50, 8, "green", "Test");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_line_chart_single_point() {
        let result = line_chart(&[100.0], 50, 8, "green", "Test");
        assert!(result.len() > 1);
    }

    #[test]
    fn test_line_chart_renders() {
        let data: Vec<f64> = (0..100)
            .map(|i| 100.0 + (i as f64 * 0.1).sin() * 10.0)
            .collect();
        let result = line_chart(&data, 40, 6, "green", "Price");
        assert!(result.len() >= 6); // at least height rows
    }

    #[test]
    fn zero_or_tiny_dimensions_do_not_panic() {
        let (o, h, l, c) = ([10.0, 11.0], [12.0, 12.5], [9.0, 10.0], [11.0, 10.5]);
        for (w, ht) in [(0, 0), (0, 5), (5, 0), (1, 1), (1, 5), (5, 1)] {
            let lc = line_chart(&[10.0, 20.0], w, ht, "green", "T");
            assert!(!lc.is_empty());
            let dc = dual_line_chart(&[10.0, 20.0], &[12.0, 18.0], w, ht, "T", "A", "B");
            assert!(!dc.is_empty());
            let cc = candlestick_chart(&o, &h, &l, &c, w, ht);
            assert!(!cc.is_empty());
        }
        // The zero cases say why nothing was drawn.
        assert_eq!(line_chart(&[10.0, 20.0], 0, 0, "green", "T"), [TOO_SMALL]);
        assert_eq!(
            dual_line_chart(&[10.0], &[20.0], 0, 0, "T", "A", "B"),
            [TOO_SMALL]
        );
        assert_eq!(
            candlestick_chart(&[10.0], &[12.0], &[9.0], &[11.0], 0, 0),
            [TOO_SMALL]
        );
        // One candle needs two columns; a width of 1 draws none.
        assert_eq!(candlestick_chart(&o, &h, &l, &c, 1, 4), [TOO_SMALL]);
        // The smallest drawable sizes still render rows.
        assert!(line_chart(&[10.0, 20.0], 1, 1, "green", "").len() >= 2);
        assert!(candlestick_chart(&o, &h, &l, &c, 2, 1).len() >= 3);
    }

    #[test]
    fn test_dual_line_chart() {
        let a: Vec<f64> = (0..50).map(|i| 100.0 + i as f64).collect();
        let b: Vec<f64> = (0..50).map(|i| 110.0 + i as f64 * 0.8).collect();
        let result = dual_line_chart(&a, &b, 40, 6, "Test", "A", "B");
        assert!(result.len() >= 6);
    }

    #[test]
    fn test_volume_bars() {
        let vols = vec![100, 200, 150, 300, 250, 180, 220];
        let result = volume_bars(&vols, 7, 3);
        assert_eq!(result.len(), 3);
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn test_volume_bars_stack_bottom_up() {
        // One column at max volume, one at half: the max bar fills every row,
        // the half bar fills only the bottom row.
        let rows: Vec<String> = volume_bars(&[100, 50], 2, 2)
            .iter()
            .map(|r| strip_ansi(r))
            .collect();
        assert_eq!(rows, vec!["  █ ", "  ██"]);
        // Fewer data points than width: no padded empty columns.
        let rows = volume_bars(&[10, 20, 30], 40, 1);
        assert_eq!(strip_ansi(&rows[0]).chars().count(), 2 + 3);
    }

    #[test]
    fn test_rsi_gauge() {
        let result = rsi_gauge(50.0);
        assert!(!result.is_empty());
        let result_low = rsi_gauge(20.0);
        assert!(!result_low.is_empty());
        let result_high = rsi_gauge(80.0);
        assert!(!result_high.is_empty());
    }

    #[test]
    fn allocation_chart_gives_each_holding_its_own_bar() {
        let items = vec![("SMALL", 20.0), ("BIG", 50.0), ("MID", 30.0)];
        let rows: Vec<String> = allocation_chart(&items)
            .iter()
            .map(|row| strip_ansi(row))
            .collect();

        assert_eq!(rows.len(), 3);
        // Heaviest first, whatever order the holdings were collected in.
        assert!(rows[0].starts_with("  BIG"));
        assert!(rows[1].starts_with("  MID"));
        assert!(rows[2].starts_with("  SMALL"));
        assert!(rows[0].ends_with(&"█".repeat(20)));
        assert!(rows[1].ends_with(&"█".repeat(12)));
        assert!(rows[2].ends_with(&"█".repeat(8)));
    }

    #[test]
    fn allocation_chart_aligns_a_long_symbol_and_needs_no_colour() {
        let items = vec![("ABCDEFGHIJKLMNO", 60.0), ("TCS.NS", 40.0)];
        let rows: Vec<String> = allocation_chart(&items)
            .iter()
            .map(|row| strip_ansi(row))
            .collect();

        let pct_at = |row: &str| row.find('%').unwrap();
        assert_eq!(pct_at(&rows[0]), pct_at(&rows[1]));
        assert!(rows[0].starts_with("  ABCDEFGHIJKLMNO"));
        assert!(rows[1].starts_with("  TCS.NS"));
    }

    #[test]
    fn test_braille_canvas_set() {
        let mut canvas = BrailleCanvas::new(5, 5);
        canvas.set(0, 0);
        canvas.set(1, 1);
        let rendered = canvas.render();
        assert_eq!(rendered.len(), 5);
    }

    #[test]
    fn test_bar_chart() {
        let items = vec![
            ("Tech", 2.5, true),
            ("Energy", -1.2, false),
            ("Finance", 0.8, true),
        ];
        let result = bar_chart(&items, 30);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn payoff_chart_shape() {
        let prices: Vec<f64> = (0..=40).map(|i| 90.0 + i as f64 * 0.5).collect();
        let expiry: Vec<f64> = prices
            .iter()
            .map(|s| (s - 100.0).clamp(0.0, 10.0) - 3.0)
            .collect();
        let today: Vec<f64> = expiry.iter().map(|v| v * 0.8).collect();
        // Compare plain text: colours are on when tests run under a terminal
        // (e.g. Nix builds), and escape codes would split the labels.
        let lines: Vec<String> = payoff_chart(&prices, &expiry, Some(&today), 101.0, 40, 8)
            .iter()
            .map(|l| strip_ansi(l))
            .collect();
        assert_eq!(lines.len(), 8 + 2, "rows, axis line, price labels");
        assert!(lines[0].contains("7.00"), "{lines:?}");
        assert!(lines[7].contains("-3.00"), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("         0 ")),
            "zero is labelled"
        );
        let labels = lines.last().unwrap();
        assert!(
            labels.contains("90") && labels.contains("110") && labels.contains("▲101"),
            "{labels}"
        );
        assert_eq!(
            payoff_chart(&prices, &expiry[1..], None, 101.0, 40, 8).len(),
            1
        );
    }
}
