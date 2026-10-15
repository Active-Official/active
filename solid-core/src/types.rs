//! Type system primitives and first-class units

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    // Time
    Nanoseconds,
    Microseconds,
    Milliseconds,
    Seconds,
    Minutes,
    Hours,
    Days,
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

            other => Some(Unit::Custom(other.to_string())),
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
pub enum SolidType {
    Null,
    Bool,
    Int,
    Float,
    String,
    Duration,
    Bytes,
    Array(Box<SolidType>),
    Table,
    Enum(Vec<String>),
    Secret,
    Any,
}
