//! Type system primitives and first-class units

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    // Time
    Nanoseconds,
    Microseconds,
    Milliseconds,
    Seconds,
    Minutes,
    Hours,
    Days,
    Weeks,
    // Bytes (binary)
    Bytes,
    Kibibytes,
    Mebibytes,
    Gibibytes,
    Tebibytes,
    // Bytes (decimal aliases)
    Kilobytes,
    Megabytes,
    Gigabytes,
    Terabytes,
    // Rate
    PerSecond,
    PerMinute,
    PerHour,
    RequestsPerSecond,
    RequestsPerMinute,
    RequestsPerHour,
    // Percent (dimensionless ratio, 100% = 1.0)
    Percent,
    // Frequency (canonical hertz; SI spellings, case-sensitive)
    Hertz,
    Kilohertz,
    Megahertz,
    Gigahertz,
    Terahertz,
    // Bits (canonical bits; case-distinct from bytes: b≠B, Mb≠MB)
    Bits,
    Kilobits,
    Megabits,
    Gigabits,
    // Data rates (canonical bits/sec; MB/s converts through megabytes)
    BitsPerSecond,
    KilobitsPerSecond,
    MegabitsPerSecond,
    GigabitsPerSecond,
    MegabytesPerSecond,
    // AI-age units: token counts, inference throughput, compute (FLOPS).
    Tokens,
    KiloTokens,
    MegaTokens,
    TokensPerSecond,
    Flops,
    TeraFlops,
    PetaFlops,
    // Custom
    Custom(String),
}

impl Unit {
    pub fn from_suffix(s: &str) -> Option<Self> {
        match s {
            "ns" => Some(Unit::Nanoseconds),
            "us" | "µs" => Some(Unit::Microseconds),
            "ms" => Some(Unit::Milliseconds),
            "s" => Some(Unit::Seconds),
            "m" => Some(Unit::Minutes),
            "h" => Some(Unit::Hours),
            "d" => Some(Unit::Days),
            "week" => Some(Unit::Weeks),
            "sec" => Some(Unit::Seconds),
            "min" => Some(Unit::Minutes),

            "B" => Some(Unit::Bytes),
            "KiB" => Some(Unit::Kibibytes),
            "MiB" => Some(Unit::Mebibytes),
            "GiB" => Some(Unit::Gibibytes),
            "TiB" => Some(Unit::Tebibytes),

            "KB" => Some(Unit::Kilobytes),
            "MB" => Some(Unit::Megabytes),
            "GB" => Some(Unit::Gigabytes),
            "TB" => Some(Unit::Terabytes),

            "/s" | "per_s" => Some(Unit::PerSecond),
            "/m" | "per_m" => Some(Unit::PerMinute),
            "/h" | "per_h" => Some(Unit::PerHour),
            "req/s" | "req_s" => Some(Unit::RequestsPerSecond),
            "/req/s" => Some(Unit::RequestsPerSecond),
            "/req/m" => Some(Unit::RequestsPerMinute),
            "/req/h" => Some(Unit::RequestsPerHour),
            "%" => Some(Unit::Percent),
            "Hz" => Some(Unit::Hertz),
            "kHz" => Some(Unit::Kilohertz),
            "MHz" => Some(Unit::Megahertz),
            "GHz" => Some(Unit::Gigahertz),
            "THz" => Some(Unit::Terahertz),
            "b" => Some(Unit::Bits),
            "Kb" => Some(Unit::Kilobits),
            "Mb" => Some(Unit::Megabits),
            "Gb" => Some(Unit::Gigabits),
            "bps" => Some(Unit::BitsPerSecond),
            "Kbps" => Some(Unit::KilobitsPerSecond),
            "Mbps" => Some(Unit::MegabitsPerSecond),
            "Gbps" => Some(Unit::GigabitsPerSecond),
            "MB/s" => Some(Unit::MegabytesPerSecond),
            "tokens" => Some(Unit::Tokens),
            "ktokens" => Some(Unit::KiloTokens),
            "Mtokens" => Some(Unit::MegaTokens),
            "tok/s" => Some(Unit::TokensPerSecond),
            "FLOPS" => Some(Unit::Flops),
            "TFLOPS" => Some(Unit::TeraFlops),
            "PFLOPS" => Some(Unit::PetaFlops),
            "req/m" | "req_m" => Some(Unit::RequestsPerMinute),
            "req/h" | "req_h" => Some(Unit::RequestsPerHour),

            other => Some(Unit::Custom(other.to_string())),
        }
    }

    /// The canonical source suffix for this unit (inverse of `from_suffix`).
    pub fn suffix(&self) -> String {
        match self {
            Unit::Nanoseconds => "ns".into(),
            Unit::Microseconds => "us".into(),
            Unit::Milliseconds => "ms".into(),
            Unit::Seconds => "s".into(),
            Unit::Minutes => "m".into(),
            Unit::Hours => "h".into(),
            Unit::Days => "d".into(),
            // `week` respells to canonical `ms` form via DurationMs values;
            // the suffix itself stays re-parseable for round-trips.
            Unit::Weeks => "week".into(),
            Unit::Bytes => "B".into(),
            Unit::Kibibytes => "KiB".into(),
            Unit::Mebibytes => "MiB".into(),
            Unit::Gibibytes => "GiB".into(),
            Unit::Tebibytes => "TiB".into(),
            Unit::Kilobytes => "KB".into(),
            Unit::Megabytes => "MB".into(),
            Unit::Gigabytes => "GB".into(),
            Unit::Terabytes => "TB".into(),
            Unit::PerSecond => "/s".into(),
            Unit::PerMinute => "/m".into(),
            Unit::PerHour => "/h".into(),
            Unit::RequestsPerSecond => "req/s".into(),
            Unit::RequestsPerMinute => "req/m".into(),
            Unit::RequestsPerHour => "req/h".into(),
            Unit::Percent => "%".into(),
            // All frequency spellings canonicalize to Hz (mirrors us->µs:
            // one canonical spelling, always re-parseable).
            Unit::Hertz | Unit::Kilohertz | Unit::Megahertz | Unit::Gigahertz | Unit::Terahertz => {
                "Hz".into()
            }
            // Bits canonicalize to `b`; all data rates to `bps`
            // (MB/s converts through megabytes at normalization).
            Unit::Bits | Unit::Kilobits | Unit::Megabits | Unit::Gigabits => "b".into(),
            Unit::BitsPerSecond
            | Unit::KilobitsPerSecond
            | Unit::MegabitsPerSecond
            | Unit::GigabitsPerSecond
            | Unit::MegabytesPerSecond => "bps".into(),
            // Token counts canonicalize to `tokens`; throughput to `tok/s`;
            // compute to `FLOPS`.
            Unit::Tokens | Unit::KiloTokens | Unit::MegaTokens => "tokens".into(),
            Unit::TokensPerSecond => "tok/s".into(),
            Unit::Flops | Unit::TeraFlops | Unit::PetaFlops => "FLOPS".into(),
            Unit::Custom(s) => s.clone(),
        }
    }

    /// Render a quantity amount in source spelling. Percent is special:
    /// the canonical value is the ratio (100% = 1.0) but the spelled form
    /// is percent-form (`25%`, never `0.25%`), so this multiplies back.
    /// Every other unit renders `{amount}{suffix}` (spec Ch. 4 §4.2).
    pub fn format_amount(&self, amount: f64) -> String {
        match self {
            Unit::Percent => format!("{}%", amount * 100.0),
            _ => format!("{}{}", amount, self.suffix()),
        }
    }

    /// Normalize a numeric value + time unit into milliseconds.
    pub fn normalize_time_to_ms(&self, value: f64) -> Option<u64> {
        let ms = match self {
            Unit::Nanoseconds => value / 1_000_000.0,
            Unit::Microseconds => value / 1_000.0,
            Unit::Milliseconds => value,
            Unit::Seconds => value * 1_000.0,
            Unit::Minutes => value * 60_000.0,
            Unit::Hours => value * 3_600_000.0,
            Unit::Days => value * 86_400_000.0,
            Unit::Weeks => value * 604_800_000.0,
            _ => return None,
        };
        Some(ms.round().max(0.0) as u64)
    }

    /// Normalize a numeric value + byte unit into bytes.
    pub fn normalize_to_bytes(&self, value: f64) -> Option<u64> {
        let bytes = match self {
            Unit::Bytes => value,
            Unit::Kibibytes => value * 1024.0,
            Unit::Mebibytes => value * 1024.0 * 1024.0,
            Unit::Gibibytes => value * 1024.0 * 1024.0 * 1024.0,
            Unit::Tebibytes => value * 1024.0 * 1024.0 * 1024.0 * 1024.0,
            Unit::Kilobytes => value * 1000.0,
            Unit::Megabytes => value * 1000.0 * 1000.0,
            Unit::Gigabytes => value * 1000.0 * 1000.0 * 1000.0,
            Unit::Terabytes => value * 1000.0 * 1000.0 * 1000.0 * 1000.0,
            _ => return None,
        };
        Some(bytes.round().max(0.0) as u64)
    }
}

#[derive(Debug, Clone)]
pub enum Constraint {
    MinMax { min: Option<i64>, max: Option<i64> },
    Enum(Vec<String>),
    Pattern(String),
    Odd,
    Custom(String),
}

#[derive(Debug, Clone)]
pub enum ActiveType {
    Null,
    Bool,
    Int,
    Float,
    String,
    Duration,
    Bytes,
    Table { closed: bool },
    Enum(Vec<String>),
    Secret,
    Any,
}

impl ActiveType {
    /// Bare type names valid without arguments (spec §2.10).
    /// `Array` needs `<T>`, `Enum` needs `(…)` — both are E020 bare.
    /// `Table` bare means any open table.
    pub fn bare(name: &str) -> Option<ActiveType> {
        match name {
            "Null" => Some(ActiveType::Null),
            "Bool" => Some(ActiveType::Bool),
            "Int" => Some(ActiveType::Int),
            "Float" => Some(ActiveType::Float),
            "String" => Some(ActiveType::String),
            "Duration" => Some(ActiveType::Duration),
            "Bytes" => Some(ActiveType::Bytes),
            "Table" => Some(ActiveType::Table { closed: false }),
            "Secret" => Some(ActiveType::Secret),
            "Any" => Some(ActiveType::Any),
            _ => None,
        }
    }
}

impl ActiveType {
    /// Human-readable name for diagnostics and documentation.
    pub fn name(&self) -> String {
        match self {
            ActiveType::Null => "Null".into(),
            ActiveType::Bool => "Bool".into(),
            ActiveType::Int => "Int".into(),
            ActiveType::Float => "Float".into(),
            ActiveType::String => "String".into(),
            ActiveType::Duration => "Duration".into(),
            ActiveType::Bytes => "Bytes".into(),
            ActiveType::Table { closed } => {
                if *closed {
                    "Table(closed)".into()
                } else {
                    "Table".into()
                }
            }
            ActiveType::Enum(values) => format!("Enum({})", values.join(", ")),
            ActiveType::Secret => "Secret".into(),
            ActiveType::Any => "Any".into(),
        }
    }
}
