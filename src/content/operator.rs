//! PDF Content Stream Operators.

/// PDF graphics and text operators.
#[derive(Debug, Clone, PartialEq)]
pub enum Operator {
    // Graphics state operators
    /// q - Save graphics state
    SaveState,
    /// Q - Restore graphics state
    RestoreState,
    /// cm - Concatenate matrix
    ConcatMatrix(f64, f64, f64, f64, f64, f64),
    /// w - Set line width
    SetLineWidth(f64),
    /// J - Set line cap style
    SetLineCap(i32),
    /// j - Set line join style
    SetLineJoin(i32),
    /// M - Set miter limit
    SetMiterLimit(f64),
    /// d - Set dash pattern
    SetDashPattern(Vec<f64>, f64),

    // Color operators
    /// G - Set gray for stroking
    SetGrayStroke(f64),
    /// g - Set gray for filling
    SetGrayFill(f64),
    /// RG - Set RGB for stroking
    SetRgbStroke(f64, f64, f64),
    /// rg - Set RGB for filling
    SetRgbFill(f64, f64, f64),
    /// K - Set CMYK for stroking
    SetCmykStroke(f64, f64, f64, f64),
    /// k - Set CMYK for filling
    SetCmykFill(f64, f64, f64, f64),

    // Path construction operators
    /// m - Move to
    MoveTo(f64, f64),
    /// l - Line to
    LineTo(f64, f64),
    /// c - Cubic Bezier curve
    CurveTo(f64, f64, f64, f64, f64, f64),
    /// v - Cubic Bezier curve (first control point replicated)
    CurveToV(f64, f64, f64, f64),
    /// y - Cubic Bezier curve (second control point replicated)
    CurveToY(f64, f64, f64, f64),
    /// h - Close path
    ClosePath,
    /// re - Rectangle
    Rectangle(f64, f64, f64, f64),

    // Path painting operators
    /// S - Stroke path
    Stroke,
    /// s - Close and stroke path
    CloseAndStroke,
    /// f - Fill path (non-zero winding)
    Fill,
    /// f* - Fill path (even-odd)
    FillEvenOdd,
    /// B - Fill and stroke (non-zero winding)
    FillAndStroke,
    /// B* - Fill and stroke (even-odd)
    FillAndStrokeEvenOdd,
    /// b - Close, fill, and stroke (non-zero winding)
    CloseFillAndStroke,
    /// b* - Close, fill, and stroke (even-odd)
    CloseFillAndStrokeEvenOdd,
    /// n - End path without filling or stroking
    EndPath,

    // Clipping operators
    /// W - Set clipping path (non-zero winding)
    Clip,
    /// W* - Set clipping path (even-odd)
    ClipEvenOdd,

    // Text object operators
    /// BT - Begin text object
    BeginText,
    /// ET - End text object
    EndText,

    // Text state operators
    /// Tc - Set character spacing
    SetCharacterSpacing(f64),
    /// Tw - Set word spacing
    SetWordSpacing(f64),
    /// Tz - Set horizontal scaling
    SetHorizontalScaling(f64),
    /// TL - Set leading
    SetLeading(f64),
    /// Tf - Set font and size
    SetFont(String, f64),
    /// Tr - Set text rendering mode
    SetTextRenderingMode(i32),
    /// Ts - Set text rise
    SetTextRise(f64),

    // Text positioning operators
    /// Td - Move text position
    MoveText(f64, f64),
    /// TD - Move text position and set leading
    MoveTextSetLeading(f64, f64),
    /// Tm - Set text matrix
    SetTextMatrix(f64, f64, f64, f64, f64, f64),
    /// T* - Move to start of next line
    NextLine,

    // Text showing operators
    /// Tj - Show text string
    ShowText(String),
    /// TJ - Show text with positioning
    ShowTextPositioned(Vec<TextElement>),
    /// ' - Move to next line and show text
    NextLineShowText(String),
    /// " - Set spacing, move to next line, and show text
    NextLineShowTextSpacing(f64, f64, String),

    // XObject operators
    /// Do - Paint XObject
    PaintXObject(String),

    // Raw operator (for custom operators)
    Raw(String),
}

/// An element in a TJ array (text or positioning).
#[derive(Debug, Clone, PartialEq)]
pub enum TextElement {
    /// A text string.
    Text(String),
    /// A positioning adjustment (negative = move right).
    Position(f64),
}

impl Operator {
    /// Converts the operator to its PDF string representation.
    pub fn to_pdf_string(&self) -> String {
        match self {
            // Graphics state
            Operator::SaveState => "q".into(),
            Operator::RestoreState => "Q".into(),
            Operator::ConcatMatrix(a, b, c, d, e, f) => {
                format!(
                    "{} {} {} {} {} {} cm",
                    fmt(a),
                    fmt(b),
                    fmt(c),
                    fmt(d),
                    fmt(e),
                    fmt(f)
                )
            }
            Operator::SetLineWidth(w) => format!("{} w", fmt(w)),
            Operator::SetLineCap(cap) => format!("{} J", cap),
            Operator::SetLineJoin(join) => format!("{} j", join),
            Operator::SetMiterLimit(limit) => format!("{} M", fmt(limit)),
            Operator::SetDashPattern(array, phase) => {
                let arr: Vec<String> = array.iter().map(fmt).collect();
                format!("[{}] {} d", arr.join(" "), fmt(phase))
            }

            // Color
            Operator::SetGrayStroke(g) => format!("{} G", fmt(g)),
            Operator::SetGrayFill(g) => format!("{} g", fmt(g)),
            Operator::SetRgbStroke(r, g, b) => format!("{} {} {} RG", fmt(r), fmt(g), fmt(b)),
            Operator::SetRgbFill(r, g, b) => format!("{} {} {} rg", fmt(r), fmt(g), fmt(b)),
            Operator::SetCmykStroke(c, m, y, k) => {
                format!("{} {} {} {} K", fmt(c), fmt(m), fmt(y), fmt(k))
            }
            Operator::SetCmykFill(c, m, y, k) => {
                format!("{} {} {} {} k", fmt(c), fmt(m), fmt(y), fmt(k))
            }

            // Path construction
            Operator::MoveTo(x, y) => format!("{} {} m", fmt(x), fmt(y)),
            Operator::LineTo(x, y) => format!("{} {} l", fmt(x), fmt(y)),
            Operator::CurveTo(x1, y1, x2, y2, x3, y3) => {
                format!(
                    "{} {} {} {} {} {} c",
                    fmt(x1),
                    fmt(y1),
                    fmt(x2),
                    fmt(y2),
                    fmt(x3),
                    fmt(y3)
                )
            }
            Operator::CurveToV(x2, y2, x3, y3) => {
                format!("{} {} {} {} v", fmt(x2), fmt(y2), fmt(x3), fmt(y3))
            }
            Operator::CurveToY(x1, y1, x3, y3) => {
                format!("{} {} {} {} y", fmt(x1), fmt(y1), fmt(x3), fmt(y3))
            }
            Operator::ClosePath => "h".into(),
            Operator::Rectangle(x, y, w, h) => {
                format!("{} {} {} {} re", fmt(x), fmt(y), fmt(w), fmt(h))
            }

            // Path painting
            Operator::Stroke => "S".into(),
            Operator::CloseAndStroke => "s".into(),
            Operator::Fill => "f".into(),
            Operator::FillEvenOdd => "f*".into(),
            Operator::FillAndStroke => "B".into(),
            Operator::FillAndStrokeEvenOdd => "B*".into(),
            Operator::CloseFillAndStroke => "b".into(),
            Operator::CloseFillAndStrokeEvenOdd => "b*".into(),
            Operator::EndPath => "n".into(),

            // Clipping
            Operator::Clip => "W".into(),
            Operator::ClipEvenOdd => "W*".into(),

            // Text object
            Operator::BeginText => "BT".into(),
            Operator::EndText => "ET".into(),

            // Text state
            Operator::SetCharacterSpacing(s) => format!("{} Tc", fmt(s)),
            Operator::SetWordSpacing(s) => format!("{} Tw", fmt(s)),
            Operator::SetHorizontalScaling(s) => format!("{} Tz", fmt(s)),
            Operator::SetLeading(l) => format!("{} TL", fmt(l)),
            Operator::SetFont(name, size) => format!("/{} {} Tf", name, fmt(size)),
            Operator::SetTextRenderingMode(mode) => format!("{} Tr", mode),
            Operator::SetTextRise(rise) => format!("{} Ts", fmt(rise)),

            // Text positioning
            Operator::MoveText(x, y) => format!("{} {} Td", fmt(x), fmt(y)),
            Operator::MoveTextSetLeading(x, y) => format!("{} {} TD", fmt(x), fmt(y)),
            Operator::SetTextMatrix(a, b, c, d, e, f) => {
                format!(
                    "{} {} {} {} {} {} Tm",
                    fmt(a),
                    fmt(b),
                    fmt(c),
                    fmt(d),
                    fmt(e),
                    fmt(f)
                )
            }
            Operator::NextLine => "T*".into(),

            // Text showing
            Operator::ShowText(s) => format!("({}) Tj", escape_string(s)),
            Operator::ShowTextPositioned(elements) => {
                let mut parts = Vec::new();
                for elem in elements {
                    match elem {
                        TextElement::Text(s) => parts.push(format!("({})", escape_string(s))),
                        TextElement::Position(p) => parts.push(fmt(p)),
                    }
                }
                format!("[{}] TJ", parts.join(" "))
            }
            Operator::NextLineShowText(s) => format!("({}) '", escape_string(s)),
            Operator::NextLineShowTextSpacing(aw, ac, s) => {
                format!("{} {} ({}) \"", fmt(aw), fmt(ac), escape_string(s))
            }

            // XObject
            Operator::PaintXObject(name) => format!("/{} Do", name),

            // Raw
            Operator::Raw(s) => s.clone(),
        }
    }
}

/// Formats a float for PDF output.
fn fmt(v: &f64) -> String {
    if *v == 0.0 {
        "0".into()
    } else if v.fract() == 0.0 && v.abs() < i64::MAX as f64 {
        (*v as i64).to_string()
    } else {
        let s = format!("{:.4}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Escapes special characters in a PDF string.
fn escape_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '(' => result.push_str("\\("),
            ')' => result.push_str("\\)"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            _ => result.push(c),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_restore() {
        assert_eq!(Operator::SaveState.to_pdf_string(), "q");
        assert_eq!(Operator::RestoreState.to_pdf_string(), "Q");
    }

    #[test]
    fn test_color_operators() {
        assert_eq!(Operator::SetGrayFill(0.5).to_pdf_string(), "0.5 g");
        assert_eq!(Operator::SetRgbFill(1.0, 0.0, 0.0).to_pdf_string(), "1 0 0 rg");
    }

    #[test]
    fn test_path_operators() {
        assert_eq!(Operator::MoveTo(100.0, 200.0).to_pdf_string(), "100 200 m");
        assert_eq!(Operator::LineTo(300.0, 400.0).to_pdf_string(), "300 400 l");
        assert_eq!(
            Operator::Rectangle(10.0, 20.0, 30.0, 40.0).to_pdf_string(),
            "10 20 30 40 re"
        );
    }

    #[test]
    fn test_text_operators() {
        assert_eq!(Operator::BeginText.to_pdf_string(), "BT");
        assert_eq!(Operator::EndText.to_pdf_string(), "ET");
        assert_eq!(
            Operator::SetFont("F1".to_string(), 12.0).to_pdf_string(),
            "/F1 12 Tf"
        );
        assert_eq!(
            Operator::ShowText("Hello".to_string()).to_pdf_string(),
            "(Hello) Tj"
        );
    }

    #[test]
    fn test_escape_string() {
        assert_eq!(escape_string("Hello"), "Hello");
        assert_eq!(escape_string("Hello (World)"), "Hello \\(World\\)");
        assert_eq!(escape_string("Line1\nLine2"), "Line1\\nLine2");
    }
}
