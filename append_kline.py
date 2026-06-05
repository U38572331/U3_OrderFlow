import os

with open(r'src/chart/kline.rs', 'a', encoding='utf-8') as f:
    f.write('''
fn draw_candle_stats(
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    x_position: f32,
    kline: &Kline,
    trades: &data::chart::kline::KlineTrades,
    text_size: f32,
    palette: &Extended,
    show_text: bool,
) {
    if !show_text {
        return;
    }

    let total_vol = trades.total_qty();
    let delta = trades.delta_qty();

    if total_vol == Qty::zero() {
        return;
    }

    let delta_pct = (f32::from(delta) / f32::from(total_vol)) * 100.0;
    
    let text_color = palette.background.weakest.text;
    let delta_color = if delta >= Qty::zero() {
        palette.success.base.color
    } else {
        palette.danger.base.color
    };

    let y_high = price_to_y(kline.high);
    let mut current_y = y_high - (text_size * 1.2);

    let stats = [
        (format!("{}", data::util::abbr_large_numbers(f32::from(total_vol))), text_color),
        (format!("D:{}", data::util::abbr_large_numbers(f32::from(delta))), delta_color),
        (format!("{:+.1}%", delta_pct), delta_color),
    ];

    for (text, color) in stats.iter().rev() {
        draw_cluster_text(
            frame,
            text,
            Point::new(x_position, current_y),
            text_size * 0.9,
            *color,
            Alignment::Center,
            Alignment::End,
        );
        current_y -= text_size * 1.2;
    }
}
''')
