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
    output.push(format!(
        "  {} {}",
        " ".repeat(label_width - 2),
        "└".to_string() + &"─".repeat(width)
    ).dimmed().to_string());

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
        format!("  {}  {}", " ".repeat(8), "└".to_string() + &"─".repeat(width))
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

    let max_vol = *volumes.iter().max().unwrap_or(&1) as f64;
    let blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    // Downsample to fit width
    let step = (volumes.len() as f64 / width as f64).max(1.0);
    let sampled: Vec<u64> = (0..width)
        .map(|i| {
            let start = (i as f64 * step) as usize;
            let end = ((i + 1) as f64 * step) as usize;
            let end = end.min(volumes.len());
            if start >= volumes.len() {
                0
            } else {
                volumes[start..end].iter().sum::<u64>() / (end - start).max(1) as u64
            }
        })
        .collect();

    // Build rows from top to bottom
    let mut rows = Vec::new();
    for row in (0..height).rev() {
        let threshold = (row as f64 / height as f64) * max_vol;
        let mut line = String::new();
        for &vol in &sampled {
            if (vol as f64) > threshold {
                let intensity = ((vol as f64 / max_vol) * 7.0) as usize;
                line.push(blocks[intensity.min(7)]);
            } else {
                line.push(' ');
            }
        }
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
    let n = opens.len().min(highs.len()).min(lows.len()).min(closes.len());
    if n == 0 {
        return vec!["  No data".to_string()];
    }

    let all_min = lows.iter().take(n).cloned().fold(f64::INFINITY, f64::min);
    let all_max = highs.iter().take(n).cloned().fold(f64::NEG_INFINITY, f64::max);
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
    let candle_width = 2; // char + space
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
        let mut col_idx = 0;
        for &ch in row {
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
            col_idx += 1;
        }

        output.push(format!("  {} {}{}", label.dimmed(), "│".dimmed(), colored_row));
    }

    output.push(
        format!("  {}  {}", " ".repeat(8), "└".to_string() + &"─".repeat(width))
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

    format!(
        "  {}  {}  {}",
        "0".dimmed(),
        bar,
        "100".dimmed()
    )
}

/// Portfolio allocation donut/bar
pub fn allocation_chart(items: &[(&str, f64)]) -> Vec<String> {
    let total: f64 = items.iter().map(|(_, v)| v).sum();
    if total == 0.0 {
        return vec![];
    }

    let colors = [
        "green", "cyan", "yellow", "magenta", "blue", "red", "white",
    ];
    let bar_width = 40;

    let mut output = Vec::new();

    // Stacked bar
    let mut bar = String::new();
    for (i, (_, value)) in items.iter().enumerate() {
        let pct = value / total;
        let chars = (pct * bar_width as f64).round() as usize;
        let block = "█".repeat(chars.max(if *value > 0.0 { 1 } else { 0 }));
        let colored = match colors[i % colors.len()] {
            "green" => block.green().to_string(),
            "cyan" => block.cyan().to_string(),
            "yellow" => block.yellow().to_string(),
            "magenta" => block.magenta().to_string(),
            "blue" => block.blue().to_string(),
            "red" => block.red().to_string(),
            _ => block.white().to_string(),
        };
        bar.push_str(&colored);
    }
    output.push(format!("  {}", bar));
    output.push(String::new());

    // Legend
    for (i, (label, value)) in items.iter().enumerate() {
        let pct = (value / total) * 100.0;
        let dot = match colors[i % colors.len()] {
            "green" => "█".green().to_string(),
            "cyan" => "█".cyan().to_string(),
            "yellow" => "█".yellow().to_string(),
            "magenta" => "█".magenta().to_string(),
            "blue" => "█".blue().to_string(),
            "red" => "█".red().to_string(),
            _ => "█".white().to_string(),
        };
        output.push(format!("  {} {:<12} {:>6.1}%", dot, label, pct));
    }

    output
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
        let data: Vec<f64> = (0..100).map(|i| 100.0 + (i as f64 * 0.1).sin() * 10.0).collect();
        let result = line_chart(&data, 40, 6, "green", "Price");
        assert!(result.len() >= 6); // at least height rows
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
    fn test_allocation_chart() {
        let items = vec![("AAPL", 5000.0), ("MSFT", 3000.0), ("GOOG", 2000.0)];
        let result = allocation_chart(&items);
        assert!(result.len() > 3); // bar + legend items
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
        let items = vec![("Tech", 2.5, true), ("Energy", -1.2, false), ("Finance", 0.8, true)];
        let result = bar_chart(&items, 30);
        assert_eq!(result.len(), 3);
    }
}
