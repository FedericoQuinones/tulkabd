//! Conversion from Tiberius column data to the engine-independent [`Value`].

use tiberius::numeric::Numeric;
use tiberius::time::{Date, DateTime, DateTime2, DateTimeOffset, SmallDateTime, Time};
use tiberius::{ColumnData, ColumnType};
use tulka_core::Value;

/// Display name of a column type, as SQL Server spells it.
pub fn type_name(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Null => "null",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 | ColumnType::Intn => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money => "money",
        ColumnType::Money4 => "smallmoney",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::Decimaln => "decimal",
        ColumnType::Numericn => "numeric",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::BigVarBin => "varbinary",
        ColumnType::BigVarChar => "varchar",
        ColumnType::BigBinary => "binary",
        ColumnType::BigChar => "char",
        ColumnType::NVarchar => "nvarchar",
        ColumnType::NChar => "nchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::Text => "text",
        ColumnType::Image => "image",
        ColumnType::NText => "ntext",
        ColumnType::SSVariant => "sql_variant",
    }
}

/// Converts one cell. `money` and `smallmoney` arrive as a `Numeric` of
/// scale 4 holding the exact wire integer.
pub fn to_value(data: ColumnData<'static>) -> Value {
    match data {
        ColumnData::U8(v) => opt(v, |v| Value::Int(v.into())),
        ColumnData::I16(v) => opt(v, |v| Value::Int(v.into())),
        ColumnData::I32(v) => opt(v, |v| Value::Int(v.into())),
        ColumnData::I64(v) => opt(v, Value::Int),
        // Going through the shortest decimal text keeps `0.1` as `0.1`
        // instead of `0.10000000149011612`.
        ColumnData::F32(v) => opt(v, |v| {
            Value::Float(v.to_string().parse().unwrap_or(f64::from(v)))
        }),
        ColumnData::F64(v) => opt(v, Value::Float),
        ColumnData::Bit(v) => opt(v, Value::Bool),
        ColumnData::String(v) => opt(v, |v| Value::Text(v.into_owned())),
        ColumnData::Guid(v) => opt(v, |v| {
            Value::Uuid(v.hyphenated().to_string().to_ascii_uppercase())
        }),
        ColumnData::Binary(v) => opt(v, |v| Value::Bytes(v.into_owned())),
        ColumnData::Numeric(v) => opt(v, |v| Value::Decimal(numeric(v))),
        ColumnData::Xml(v) => opt(v, |v| Value::Text(v.to_string())),
        ColumnData::DateTime(v) => opt(v, |v| Value::DateTime(datetime(v))),
        ColumnData::SmallDateTime(v) => opt(v, |v| Value::DateTime(smalldatetime(v))),
        ColumnData::Time(v) => opt(v, |v| Value::Time(time(v))),
        ColumnData::Date(v) => opt(v, |v| Value::Date(date(v))),
        ColumnData::DateTime2(v) => opt(v, |v| Value::DateTime(datetime2(v))),
        ColumnData::DateTimeOffset(v) => opt(v, |v| Value::DateTimeOffset(datetimeoffset(v))),
    }
}

fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> Value) -> Value {
    v.map_or(Value::Null, f)
}

/// Exact text of a decimal, with exactly `scale` fractional digits.
fn numeric(n: Numeric) -> String {
    scaled(n.value(), n.scale())
}

/// Formats `value / 10^scale` exactly.
fn scaled(value: i128, scale: u8) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let digits = value.unsigned_abs().to_string();
    let scale = usize::from(scale);
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = digits.split_at(digits.len() - scale);
    format!("{sign}{int}.{frac}")
}

/// Converts days since 0001-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(days_since_0001: i64) -> (i64, u32, u32) {
    // Howard Hinnant's `civil_from_days`, shifted from the 1970 epoch:
    // 0001-01-01 is 719162 days before 1970-01-01.
    let z = days_since_0001 - 719_162 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days from 0001-01-01 to 1900-01-01, the epoch of `datetime` and `smalldatetime`.
const DAYS_TO_1900: i64 = 693_595;

fn format_date(days_since_0001: i64) -> String {
    let (y, m, d) = civil_from_days(days_since_0001);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `hh:mm:ss` plus `.` and `digits` fractional digits when `digits > 0`.
/// `fraction` is in units of 10^-digits seconds.
fn format_time(seconds: u64, fraction: u64, digits: u8) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if digits == 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!(
            "{h:02}:{m:02}:{s:02}.{fraction:0width$}",
            width = usize::from(digits)
        )
    }
}

fn date(d: Date) -> String {
    format_date(i64::from(d.days()))
}

/// `time(n)`, with `n` fractional digits.
fn time(t: Time) -> String {
    let unit = 10u64.pow(u32::from(t.scale()));
    format_time(t.increments() / unit, t.increments() % unit, t.scale())
}

fn datetime2(dt: DateTime2) -> String {
    format!("{} {}", date(dt.date()), time(dt.time()))
}

/// `datetimeoffset` is stored in UTC; it is shown in its own offset.
fn datetimeoffset(dto: DateTimeOffset) -> String {
    let dt = dto.datetime2();
    let offset = i64::from(dto.offset());
    let t = dt.time();
    let unit = 10u64.pow(u32::from(t.scale()));
    let utc_seconds = (t.increments() / unit) as i64;
    let fraction = t.increments() % unit;
    let local = i64::from(dt.date().days()) * 86_400 + utc_seconds + offset * 60;
    let (days, seconds) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.abs();
    format!(
        "{} {} {sign}{:02}:{:02}",
        format_date(days),
        format_time(seconds as u64, fraction, t.scale()),
        offset / 60,
        offset % 60
    )
}

/// `datetime` counts 1/300 s; SQL Server shows it rounded to milliseconds.
fn datetime(dt: DateTime) -> String {
    let ms = (u64::from(dt.seconds_fragments()) * 10 + 1) / 3;
    format!(
        "{} {}",
        format_date(DAYS_TO_1900 + i64::from(dt.days())),
        format_time(ms / 1000, ms % 1000, 3)
    )
}

/// `smalldatetime` stores minutes since midnight in `seconds_fragments`.
fn smalldatetime(dt: SmallDateTime) -> String {
    format!(
        "{} {}",
        format_date(DAYS_TO_1900 + i64::from(dt.days())),
        format_time(u64::from(dt.seconds_fragments()) * 60, 0, 0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_formats_exactly() {
        assert_eq!(scaled(891245050, 2), "8912450.50");
        assert_eq!(scaled(-5, 2), "-0.05");
        assert_eq!(scaled(42, 0), "42");
        assert_eq!(scaled(0, 3), "0.000");
        assert_eq!(scaled(i128::MIN, 4).len(), 41);
    }

    #[test]
    fn money_is_exact() {
        let money = |v: i64| {
            to_value(ColumnData::Numeric(Some(Numeric::new_with_scale(
                v.into(),
                4,
            ))))
        };
        let dec = |s: &str| Value::Decimal(s.into());
        assert_eq!(money(125_000), dec("12.5000"));
        assert_eq!(money(-32_500), dec("-3.2500"));
        assert_eq!(money(1), dec("0.0001"));
        assert_eq!(money(i64::MAX), dec("922337203685477.5807"));
        assert_eq!(money(i64::MIN), dec("-922337203685477.5808"));
    }

    #[test]
    fn dates_from_days() {
        assert_eq!(format_date(0), "0001-01-01");
        assert_eq!(format_date(DAYS_TO_1900), "1900-01-01");
        assert_eq!(format_date(DAYS_TO_1900 - 53_690), "1753-01-01");
        assert_eq!(format_date(3_652_058), "9999-12-31");
        // 2000 is a leap year.
        assert_eq!(format_date(730_178), "2000-02-29");
    }

    #[test]
    fn times() {
        assert_eq!(time(Time::new(50_580_123, 3)), "14:03:00.123");
        assert_eq!(time(Time::new(50_580, 0)), "14:03:00");
        assert_eq!(
            datetime(DateTime::new(0, 300 * 3600 + 1)),
            "1900-01-01 01:00:00.003"
        );
        assert_eq!(datetime(DateTime::new(0, 2)), "1900-01-01 00:00:00.007");
        assert_eq!(
            smalldatetime(SmallDateTime::new(1, 843)),
            "1900-01-02 14:03:00"
        );
    }

    #[test]
    fn offsets_shift_from_utc() {
        // 2026-10-20 17:03 UTC is 14:03 at -03:00.
        let days = 739_908; // 2026-10-20
        assert_eq!(format_date(days), "2026-10-20");
        let utc = DateTime2::new(Date::new(days as u32), Time::new(17 * 3600 + 180, 0));
        assert_eq!(
            datetimeoffset(DateTimeOffset::new(utc, -180)),
            "2026-10-20 14:03:00 -03:00"
        );
        // Crossing midnight backwards and a positive half-hour offset.
        let utc = DateTime2::new(Date::new(days as u32), Time::new(600, 1));
        assert_eq!(
            datetimeoffset(DateTimeOffset::new(utc, -60)),
            "2026-10-19 23:01:00.0 -01:00"
        );
        assert_eq!(
            datetimeoffset(DateTimeOffset::new(utc, 330)),
            "2026-10-20 05:31:00.0 +05:30"
        );
    }
}
