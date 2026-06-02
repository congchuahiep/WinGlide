use nu_ansi_term::Color;
use std::fmt::Write;
use tracing::Level;
use tracing_forest::printer::{Formatter, MakeStdout};
use tracing_forest::tree::{Event, Span, Tree};
use tracing_forest::Printer;

pub struct CleanFormatter;

impl Formatter for CleanFormatter {
    type Error = std::fmt::Error;

    fn fmt(&self, tree: &Tree) -> Result<String, Self::Error> {
        let mut w = String::with_capacity(256);
        Self::format_tree(tree, None, &mut Vec::new(), &mut w)?;
        Ok(w)
    }
}

enum Indent {
    Null,
    Line,
    Fork,
    Turn,
}

impl Indent {
    fn repr(&self) -> &'static str {
        match self {
            Self::Null => "   ",
            Self::Line => "│  ",
            Self::Fork => "├─ ",
            Self::Turn => "└─ ",
        }
    }
}

impl CleanFormatter {
    fn format_tree(
        tree: &Tree,
        duration_root: Option<f64>,
        indent: &mut Vec<Indent>,
        w: &mut String,
    ) -> std::fmt::Result {
        match tree {
            Tree::Event(event) => {
                write!(w, "{} ", ColorLevel(event.level()))?;
                Self::format_indent(indent, w)?;

                if let Some(prefix) = event.tag().and_then(|t| t.prefix()) {
                    let dim = Color::White.dimmed();
                    write!(w, "{}{}:{} ", dim.prefix(), prefix, dim.suffix())?;
                }

                if let Some(msg) = event.message() {
                    w.write_str(msg)?;
                }

                for f in event.fields() {
                    write!(w, " | {}: {}", f.key(), f.value())?;
                }
                writeln!(w)
            }
            Tree::Span(span) => {
                let total = span.total_duration().as_nanos() as f64;
                let inner = span.inner_duration().as_nanos() as f64;
                let root = duration_root.unwrap_or(total);

                write!(w, "{} ", ColorLevel(span.level()))?;
                Self::format_indent(indent, w)?;

                let cyan = Color::Cyan;
                let yellow = Color::Yellow;
                write!(
                    w,
                    "{}{}{} {}[ {} | ",
                    cyan.prefix(),
                    span.name(),
                    cyan.suffix(),
                    yellow.prefix(),
                    Self::fmt_dur(total)
                )?;

                if inner > 0.0 {
                    let base = span.base_duration().as_nanos() as f64;
                    write!(w, "{:.2}% / ", 100.0 * base / root)?;
                }
                write!(w, "{:.2}% ]{}", 100.0 * total / root, yellow.suffix())?;

                for f in span.fields() {
                    write!(w, " | {}: {}", f.key(), f.value())?;
                }
                writeln!(w)?;

                let nodes: Vec<_> = span.nodes().iter().collect();
                if let Some((last, rest)) = nodes.split_last() {
                    if let Some(edge) = indent.last_mut() {
                        *edge = match edge {
                            Indent::Turn => Indent::Null,
                            Indent::Fork => Indent::Line,
                            _ => Indent::Null,
                        };
                    }
                    indent.push(Indent::Fork);
                    for tree in rest {
                        if let Some(e) = indent.last_mut() {
                            *e = Indent::Fork;
                        }
                        Self::format_tree(tree, Some(root), indent, w)?;
                    }
                    if let Some(e) = indent.last_mut() {
                        *e = Indent::Turn;
                    }
                    Self::format_tree(last, Some(root), indent, w)?;
                    indent.pop();
                }
                Ok(())
            }
        }
    }

    fn format_indent(indent: &[Indent], w: &mut String) -> std::fmt::Result {
        let cyan = Color::Cyan.dimmed();

        for i in indent {
            write!(w, "\t{}{}{}", cyan.prefix(), i.repr(), cyan.suffix())?;
        }
        Ok(())
    }

    fn fmt_dur(mut t: f64) -> String {
        for unit in ["ns", "µs", "ms", "s"] {
            if t < 10.0 {
                return format!("{t:.2}{unit}");
            } else if t < 100.0 {
                return format!("{t:.1}{unit}");
            } else if t < 1000.0 {
                return format!("{t:.0}{unit}");
            }
            t /= 1000.0;
        }
        format!("{:.0}s", t * 1000.0)
    }
}

struct ColorLevel(Level);

impl std::fmt::Display for ColorLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let color = match self.0 {
            Level::TRACE => Color::Purple,
            Level::DEBUG => Color::Blue,
            Level::INFO => Color::Green,
            Level::WARN => Color::Rgb(252, 234, 160), // cam nhạt
            Level::ERROR => Color::Red,
        };
        let style = color.bold();
        write!(f, "{}{:<6}{}", style.prefix(), self.0, style.suffix())
    }
}
