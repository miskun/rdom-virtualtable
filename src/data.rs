//! The data model shared by every row source (`SPEC_DATA_SOURCE.md` §4):
//! stable row identity ([`RowKey`]), typed cells ([`CellValue`]), a [`Row`],
//! and the windowed-view change [`Delta`].
//!
//! Pure data — no DOM, no rdom-tui, no async. `CellValue` carries the
//! type-aware sort comparison used by the in-memory convenience mode; a windowed
//! source delegates sort to its backend and never calls it.

use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Opaque, cheap-to-clone stable row identity. Consumers build it from their
/// source's primary key (e.g. an Observatory `schema.primary_key` tuple joined
/// into one string); the table treats it as opaque and only ever compares /
/// hashes it. `Arc<str>` keeps clone + hash O(1) for the selection set and the
/// window buffer's key index.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RowKey(Arc<str>);

impl RowKey {
    pub fn new(s: impl Into<Arc<str>>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for RowKey {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<String> for RowKey {
    fn from(s: String) -> Self {
        Self(Arc::from(s.as_str()))
    }
}

impl std::fmt::Debug for RowKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RowKey({:?})", &*self.0)
    }
}

impl std::fmt::Display for RowKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A status severity a [`CellValue::Status`] (or a row's status cell) carries.
/// The renderer maps it to a CSS-targetable attribute; the consumer decides
/// what each level means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusLevel {
    Ok,
    Info,
    Warn,
    Error,
}

impl StatusLevel {
    /// The `data-vt-status` attribute value the renderer stamps.
    pub fn as_attr(self) -> &'static str {
        match self {
            StatusLevel::Ok => "ok",
            StatusLevel::Info => "info",
            StatusLevel::Warn => "warn",
            StatusLevel::Error => "error",
        }
    }
}

/// A typed cell value. `Text` is the default / fallback and what a bare string
/// becomes. The type drives rich rendering AND the in-memory sort comparison;
/// extend the enum as real columns demand (Progress / Badge / Link are
/// deliberately deferred — `SPEC_DATA_SOURCE.md` §12).
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Empty,
    Text(String),
    /// Unit-less number; formatting (precision, unit suffix) is the renderer's
    /// or consumer's job — the model only needs the value for sort + display.
    Number(f64),
    /// A byte count, rendered human-readable (`1.5 KiB`); compared numerically.
    Bytes(u64),
    /// A duration (e.g. resource age), rendered compactly (`3d4h`); compared by
    /// length.
    Duration(Duration),
    /// Text plus a severity the renderer can colour.
    Status {
        text: String,
        level: StatusLevel,
    },
}

impl CellValue {
    /// The display string for this cell (what the `<td>` text node shows).
    pub fn display(&self) -> String {
        match self {
            CellValue::Empty => String::new(),
            CellValue::Text(s) => s.clone(),
            CellValue::Number(n) => format_number(*n),
            CellValue::Bytes(b) => format_bytes(*b),
            CellValue::Duration(d) => format_duration(*d),
            CellValue::Status { text, .. } => text.clone(),
        }
    }

    /// The status level this cell carries, if any (drives the row/cell status
    /// attribute).
    pub fn status(&self) -> Option<StatusLevel> {
        match self {
            CellValue::Status { level, .. } => Some(*level),
            _ => None,
        }
    }

    /// Type-aware ordering for the in-memory sort. Same-typed values compare by
    /// their natural order (numeric, byte count, duration, text); mixed types
    /// fall back to comparing their display strings, and `Empty` sorts before
    /// everything. A windowed source never calls this — it sorts server-side.
    pub fn sort_cmp(&self, other: &CellValue) -> Ordering {
        use CellValue::*;
        match (self, other) {
            (Empty, Empty) => Ordering::Equal,
            (Empty, _) => Ordering::Less,
            (_, Empty) => Ordering::Greater,
            (Number(a), Number(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Bytes(a), Bytes(b)) => a.cmp(b),
            (Duration(a), Duration(b)) => a.cmp(b),
            // Text vs Text, or any other same-ish pair, and all cross-type
            // pairs: compare display strings (numeric-aware for two texts that
            // both parse, so "2" < "10").
            (a, b) => {
                let (sa, sb) = (a.display(), b.display());
                match (sa.trim().parse::<f64>(), sb.trim().parse::<f64>()) {
                    (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                    _ => sa.cmp(&sb),
                }
            }
        }
    }
}

impl From<&str> for CellValue {
    fn from(s: &str) -> Self {
        CellValue::Text(s.to_string())
    }
}

impl From<String> for CellValue {
    fn from(s: String) -> Self {
        CellValue::Text(s)
    }
}

/// One row: stable identity + cells in the table's column order.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: RowKey,
    pub cells: Vec<CellValue>,
}

impl Row {
    pub fn new(key: impl Into<RowKey>, cells: Vec<CellValue>) -> Self {
        Self {
            key: key.into(),
            cells,
        }
    }

    /// The cell at `col`, or `&CellValue::Empty` if the row is short.
    pub fn cell(&self, col: usize) -> &CellValue {
        self.cells.get(col).unwrap_or(&CellValue::Empty)
    }
}

/// A change to the windowed view — mirrors Observatory's `Delta` 1:1 so the
/// consumer adapter is a straight map. The `epoch` it applies to is passed
/// alongside it to [`apply`](crate::VirtualTableView::apply), not stored here.
#[derive(Clone, Debug)]
pub enum Delta {
    /// Full snapshot for `start..start + rows.len()` (Observatory `Resync`).
    /// Replaces the window buffer for that range.
    Resync { start: usize, rows: Vec<Row> },
    /// Rows changed or entered the window — replace/insert by [`RowKey`].
    Upsert { rows: Vec<Row> },
    /// Rows left the window — drop by [`RowKey`].
    Remove { keys: Vec<RowKey> },
}

// ── display formatters (lean; consumers can pre-format into Text instead) ──

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn format_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{}s", s / 60, s % 60)
    } else if s < 86400 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d{}h", s / 86400, (s % 86400) / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_key_clone_and_eq() {
        let a: RowKey = "ns\u{1}pod-1".into();
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "ns\u{1}pod-1");
        let c: RowKey = String::from("other").into();
        assert_ne!(a, c);
    }

    #[test]
    fn str_becomes_text_cell() {
        assert_eq!(CellValue::from("hi"), CellValue::Text("hi".into()));
        assert_eq!(
            CellValue::from(String::from("yo")),
            CellValue::Text("yo".into())
        );
    }

    #[test]
    fn display_strings() {
        assert_eq!(CellValue::Empty.display(), "");
        assert_eq!(CellValue::Text("x".into()).display(), "x");
        assert_eq!(CellValue::Number(42.0).display(), "42");
        assert_eq!(CellValue::Bytes(2048).display(), "2.0 KiB");
        assert_eq!(CellValue::Bytes(512).display(), "512 B");
        assert_eq!(
            CellValue::Duration(Duration::from_secs(90)).display(),
            "1m30s"
        );
        assert_eq!(
            CellValue::Status {
                text: "Running".into(),
                level: StatusLevel::Ok
            }
            .display(),
            "Running"
        );
    }

    #[test]
    fn sort_cmp_numeric_and_text() {
        // Two Numbers compare by value.
        assert_eq!(
            CellValue::Number(2.0).sort_cmp(&CellValue::Number(10.0)),
            Ordering::Less
        );
        // Two Texts that both parse compare numerically ("2" < "10").
        assert_eq!(
            CellValue::from("2").sort_cmp(&CellValue::from("10")),
            Ordering::Less
        );
        // Plain text is lexicographic.
        assert_eq!(
            CellValue::from("apple").sort_cmp(&CellValue::from("banana")),
            Ordering::Less
        );
        // Bytes compare by value, not display string ("2.0 KiB" vs "512 B").
        assert_eq!(
            CellValue::Bytes(512).sort_cmp(&CellValue::Bytes(2048)),
            Ordering::Less
        );
        // Empty sorts first.
        assert_eq!(
            CellValue::Empty.sort_cmp(&CellValue::from("a")),
            Ordering::Less
        );
    }

    #[test]
    fn row_cell_access_is_clamped() {
        let r = Row::new("k", vec![CellValue::from("a")]);
        assert_eq!(r.cell(0), &CellValue::Text("a".into()));
        assert_eq!(r.cell(5), &CellValue::Empty);
    }

    #[test]
    fn status_level_attr() {
        assert_eq!(StatusLevel::Warn.as_attr(), "warn");
        assert_eq!(
            CellValue::Status {
                text: "x".into(),
                level: StatusLevel::Error
            }
            .status(),
            Some(StatusLevel::Error)
        );
        assert_eq!(CellValue::from("x").status(), None);
    }
}
